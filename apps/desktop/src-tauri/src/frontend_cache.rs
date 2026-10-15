//! Caches that live in the webview (tile bitmaps today; decoded preview
//! images later) and count against the memory governor (§5.5).
//!
//! The frontend reports each cache's size in bytes; the governor sees it like
//! any other registration. When the governor needs memory back, `evict` sends
//! a `memory-trim` event to the webview and charges the requested amount at
//! once, so the governor doesn't keep evicting other caches while the webview
//! catches up. The webview's next report sets the real size again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use quire_core::memory::{CacheClass, DocId, Evictable, MemoryGovernor, Registration};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Where trim requests go. The app emits a Tauri event; tests record them.
pub trait TrimSink: Send + Sync {
    fn trim(&self, cache: &str, key: u64, bytes: u64);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrimEvent<'a> {
    cache: &'a str,
    key: u64,
    bytes: u64,
}

pub struct EventSink(pub AppHandle);

impl TrimSink for EventSink {
    fn trim(&self, cache: &str, key: u64, bytes: u64) {
        if let Err(e) = self.0.emit("memory-trim", TrimEvent { cache, key, bytes }) {
            tracing::warn!(%e, "memory-trim event failed");
        }
    }
}

struct Entry {
    cache: &'static str,
    key: u64,
    /// The size the governor currently counts.
    bytes: AtomicU64,
    sink: Arc<dyn TrimSink>,
    registration: OnceLock<Registration>,
}

impl Entry {
    /// No lock is held here: a charge may call straight back into `evict`.
    fn charge(&self, delta: i64) {
        if let Some(r) = self.registration.get() {
            r.charge(delta);
        }
    }

    fn set(&self, bytes: u64) {
        let old = self.bytes.swap(bytes, Ordering::AcqRel);
        self.charge(bytes as i64 - old as i64);
    }
}

impl Evictable for Entry {
    fn evict(&self, bytes: u64) -> u64 {
        let mut current = self.bytes.load(Ordering::Acquire);
        let freed = loop {
            let freed = bytes.min(current);
            match self.bytes.compare_exchange(
                current,
                current - freed,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break freed,
                Err(now) => current = now,
            }
        };
        if freed > 0 {
            self.sink.trim(self.cache, self.key, freed);
            self.charge(-(freed as i64));
        }
        freed
    }
}

/// The registry of webview caches, keyed by (cache name, key), e.g.
/// ("tile-bitmaps", view id).
pub struct FrontendCaches {
    governor: MemoryGovernor,
    sink: Arc<dyn TrimSink>,
    entries: Mutex<HashMap<(&'static str, u64), Arc<Entry>>>,
}

/// Cache names the frontend may report, and their eviction class.
fn known(cache: &str) -> Option<(&'static str, CacheClass)> {
    match cache {
        "tile-bitmaps" => Some(("tile-bitmaps", CacheClass::Raster)),
        "preview-images" => Some(("preview-images", CacheClass::DecodedImage)),
        "md-preview-dom" => Some(("md-preview-dom", CacheClass::Layout)),
        // Rendered math and diagrams, shared by all documents (ADR-0033).
        "md-rich" => Some(("md-rich", CacheClass::Layout)),
        _ => None,
    }
}

impl FrontendCaches {
    pub fn new(governor: MemoryGovernor, sink: Arc<dyn TrimSink>) -> Self {
        Self {
            governor,
            sink,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Sets a webview cache's current size.
    pub fn report(
        &self,
        cache: &str,
        key: u64,
        doc: Option<u64>,
        bytes: u64,
    ) -> Result<(), String> {
        let (name, class) = known(cache).ok_or_else(|| format!("unknown cache {cache}"))?;
        let entry = {
            let mut entries = self.entries.lock();
            Arc::clone(entries.entry((name, key)).or_insert_with(|| {
                let entry = Arc::new(Entry {
                    cache: name,
                    key,
                    bytes: AtomicU64::new(0),
                    sink: Arc::clone(&self.sink),
                    registration: OnceLock::new(),
                });
                let weak: Weak<dyn Evictable> = Arc::downgrade(&entry) as Weak<dyn Evictable>;
                let reg = self.governor.register(name, doc.map(DocId), class, weak);
                let _ = entry.registration.set(reg);
                entry
            }))
        };
        entry.set(bytes);
        Ok(())
    }

    /// Forgets a cache (its surface closed); its bytes leave the totals.
    pub fn release(&self, cache: &str, key: u64) {
        let Some((name, _)) = known(cache) else {
            return;
        };
        // Dropping the entry drops its registration.
        self.entries.lock().remove(&(name, key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::memory::{GovernorConfig, MIB};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, u64, u64)>>);

    impl TrimSink for Recorder {
        fn trim(&self, cache: &str, key: u64, bytes: u64) {
            self.0.lock().push((cache.to_owned(), key, bytes));
        }
    }

    fn setup(global: u64) -> (MemoryGovernor, Arc<Recorder>, FrontendCaches) {
        let governor = MemoryGovernor::new(GovernorConfig {
            per_document: global,
            global,
            low_watermark: 0.5,
            background_retain: 0.25,
        });
        let rec = Arc::new(Recorder::default());
        let caches = FrontendCaches::new(governor.clone(), rec.clone());
        (governor, rec, caches)
    }

    #[test]
    fn reports_count_against_the_governor() {
        let (governor, rec, caches) = setup(100 * MIB);
        caches.report("tile-bitmaps", 7, None, 10 * MIB).unwrap();
        assert_eq!(governor.total_bytes(), 10 * MIB);
        caches.report("tile-bitmaps", 7, None, 4 * MIB).unwrap();
        assert_eq!(governor.total_bytes(), 4 * MIB);
        let stats = governor.stats();
        assert_eq!(stats.caches.len(), 1);
        assert_eq!(stats.caches[0].name, "tile-bitmaps");
        assert!(rec.0.lock().is_empty());
        caches.release("tile-bitmaps", 7);
        assert_eq!(governor.total_bytes(), 0);
        assert!(caches.report("nonsense", 1, None, 1).is_err());
    }

    #[test]
    fn going_over_budget_asks_the_webview_to_trim() {
        let (governor, rec, caches) = setup(100 * MIB);
        caches.report("tile-bitmaps", 1, None, 60 * MIB).unwrap();
        caches.report("tile-bitmaps", 2, None, 60 * MIB).unwrap();
        // Over the 100 MiB budget: shed down to the 50 MiB low watermark.
        let trims = rec.0.lock().clone();
        let asked: u64 = trims.iter().map(|t| t.2).sum();
        assert_eq!(asked, 70 * MIB, "{trims:?}");
        assert_eq!(governor.total_bytes(), 50 * MIB);
        // The webview trims and reports its real sizes.
        caches.report("tile-bitmaps", 1, None, 0).unwrap();
        caches.report("tile-bitmaps", 2, None, 50 * MIB).unwrap();
        assert_eq!(governor.total_bytes(), 50 * MIB);
    }
}
