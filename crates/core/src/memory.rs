//! Memory governor (MASTER_PROMPT §4 memory row, §5.5; M0 task 6).
//!
//! Every cache (tiles, decoded images, display lists, layouts) registers with
//! the [`MemoryGovernor`] and reports its size changes through its
//! [`Registration`]. When a document goes over its budget, or the app goes
//! over the global budget, the governor asks caches to evict, in this order:
//!
//! 1. caches of background documents before the foreground one (§4: background
//!    tabs shed their caches first);
//! 2. cheaper-to-rebuild classes first ([`CacheClass`] order);
//! 3. inside a cache, least recently used first (the cache's own policy).
//!
//! It evicts down to a low watermark below the budget, so a cache hovering at
//! the limit doesn't evict on every insert.
//!
//! Contract for cache authors: never call [`Registration::charge`] while
//! holding your cache's own lock, because the charge may call straight back
//! into [`Evictable::evict`].

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::Hash;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use serde::Serialize;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Identifies an open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct DocId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DocState {
    Foreground,
    Background,
}

/// What a cache holds, in eviction order: earlier classes are cheaper to
/// rebuild and go first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum CacheClass {
    /// Prefetched or speculative output.
    Speculative,
    /// Raster tiles.
    Raster,
    /// Decoded images.
    DecodedImage,
    /// Display lists.
    DisplayList,
    /// Layout results (most expensive to rebuild).
    Layout,
    /// Counted against the budget but never evicted (document models).
    Fixed,
}

/// A cache that can give memory back.
pub trait Evictable: Send + Sync {
    /// Frees roughly `bytes` (more or less is fine) and returns how many were
    /// freed. Implementations report the decrease through their
    /// [`Registration`] as usual.
    fn evict(&self, bytes: u64) -> u64;
}

#[derive(Clone, Debug)]
pub struct GovernorConfig {
    /// Resident memory per document beyond the mapped source (§4: 1.5 GB).
    pub per_document: u64,
    /// All caches together.
    pub global: u64,
    /// Evict down to this fraction of a budget once it's exceeded.
    pub low_watermark: f64,
    /// A document sent to the background keeps this fraction of its budget.
    pub background_retain: f64,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            per_document: 3 * GIB / 2,
            global: 3 * GIB,
            low_watermark: 0.9,
            background_retain: 0.25,
        }
    }
}

struct RegInfo {
    name: &'static str,
    doc: Option<DocId>,
    class: CacheClass,
    bytes: Arc<AtomicI64>,
    evicted: Arc<AtomicU64>,
    target: Option<Weak<dyn Evictable>>,
}

struct DocInfo {
    state: DocState,
    bytes: Arc<AtomicI64>,
}

struct Inner {
    config: Mutex<GovernorConfig>,
    total: AtomicI64,
    regs: Mutex<BTreeMap<u64, RegInfo>>,
    docs: Mutex<HashMap<DocId, DocInfo>>,
    next_id: AtomicU64,
    /// Held while rebalancing; charges made by evicting caches see it taken
    /// and don't recurse.
    rebalancing: Mutex<()>,
}

/// The global memory budget keeper. Cheap to clone.
#[derive(Clone)]
pub struct MemoryGovernor {
    inner: Arc<Inner>,
}

impl fmt::Debug for MemoryGovernor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryGovernor")
            .field("total", &self.total_bytes())
            .finish()
    }
}

/// A cache's link to the governor. Dropping it unregisters the cache and
/// releases its bytes from the totals.
pub struct Registration {
    inner: Weak<Inner>,
    id: u64,
    bytes: Arc<AtomicI64>,
    doc_bytes: Option<Arc<AtomicI64>>,
}

impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registration")
            .field("id", &self.id)
            .field("bytes", &self.bytes())
            .finish()
    }
}

/// Per-cache numbers for the performance HUD.
#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub name: &'static str,
    pub doc: Option<DocId>,
    pub class: CacheClass,
    pub bytes: u64,
    pub evicted_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct GovernorStats {
    pub total_bytes: u64,
    pub global_budget: u64,
    pub per_document_budget: u64,
    pub caches: Vec<CacheStats>,
}

fn clamp(v: i64) -> u64 {
    v.max(0) as u64
}

fn scaled(budget: u64, fraction: f64) -> u64 {
    (budget as f64 * fraction) as u64
}

impl MemoryGovernor {
    pub fn new(config: GovernorConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config: Mutex::new(config),
                total: AtomicI64::new(0),
                regs: Mutex::new(BTreeMap::new()),
                docs: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(0),
                rebalancing: Mutex::new(()),
            }),
        }
    }

    /// Changes the budgets (settings, §7) and enforces them at once.
    pub fn set_config(&self, config: GovernorConfig) {
        *self.inner.config.lock() = config;
        self.rebalance();
    }

    pub fn config(&self) -> GovernorConfig {
        self.inner.config.lock().clone()
    }

    pub fn total_bytes(&self) -> u64 {
        clamp(self.inner.total.load(Ordering::Acquire))
    }

    pub fn document_bytes(&self, doc: DocId) -> u64 {
        self.inner
            .docs
            .lock()
            .get(&doc)
            .map_or(0, |d| clamp(d.bytes.load(Ordering::Acquire)))
    }

    /// Registers an evictable cache. `doc` is `None` for app-wide caches.
    pub fn register(
        &self,
        name: &'static str,
        doc: Option<DocId>,
        class: CacheClass,
        cache: Weak<dyn Evictable>,
    ) -> Registration {
        let target = (class != CacheClass::Fixed).then_some(cache);
        self.register_inner(name, doc, class, target)
    }

    /// Registers memory that counts against the budget but can't be evicted.
    pub fn register_fixed(&self, name: &'static str, doc: Option<DocId>) -> Registration {
        self.register_inner(name, doc, CacheClass::Fixed, None)
    }

    fn register_inner(
        &self,
        name: &'static str,
        doc: Option<DocId>,
        class: CacheClass,
        target: Option<Weak<dyn Evictable>>,
    ) -> Registration {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let bytes = Arc::new(AtomicI64::new(0));
        let doc_bytes = doc.map(|d| {
            let mut docs = self.inner.docs.lock();
            let info = docs.entry(d).or_insert_with(|| DocInfo {
                state: DocState::Foreground,
                bytes: Arc::new(AtomicI64::new(0)),
            });
            Arc::clone(&info.bytes)
        });
        self.inner.regs.lock().insert(
            id,
            RegInfo {
                name,
                doc,
                class,
                bytes: Arc::clone(&bytes),
                evicted: Arc::new(AtomicU64::new(0)),
                target,
            },
        );
        Registration {
            inner: Arc::downgrade(&self.inner),
            id,
            bytes,
            doc_bytes,
        }
    }

    /// Marks a document foreground or background. Going to the background
    /// sheds its caches down to the retain fraction of its budget.
    pub fn set_document_state(&self, doc: DocId, state: DocState) {
        if let Some(info) = self.inner.docs.lock().get_mut(&doc) {
            info.state = state;
        } else {
            return;
        }
        if state == DocState::Background {
            let config = self.config();
            let target = scaled(config.per_document, config.background_retain);
            let _guard = self.inner.rebalancing.lock();
            self.shed(Some(doc), target);
        }
    }

    /// Forgets a closed document. Its caches should already be dropped.
    pub fn remove_document(&self, doc: DocId) {
        self.inner.docs.lock().remove(&doc);
    }

    /// Enforces the budgets now. Called automatically when a charge pushes
    /// usage over a budget.
    pub fn rebalance(&self) {
        let Some(_guard) = self.inner.rebalancing.try_lock() else {
            return;
        };
        let config = self.config();
        let over: Vec<DocId> = self
            .inner
            .docs
            .lock()
            .iter()
            .filter(|(_, d)| clamp(d.bytes.load(Ordering::Acquire)) > config.per_document)
            .map(|(id, _)| *id)
            .collect();
        for doc in over {
            self.shed(Some(doc), scaled(config.per_document, config.low_watermark));
        }
        if self.total_bytes() > config.global {
            self.shed(None, scaled(config.global, config.low_watermark));
        }
    }

    /// Evicts from caches in `scope` (one document, or everything) until its
    /// usage is at most `target`. Caller holds the rebalancing lock.
    fn shed(&self, scope: Option<DocId>, target: u64) {
        let usage = || match scope {
            Some(doc) => self.document_bytes(doc),
            None => self.total_bytes(),
        };
        if usage() <= target {
            return;
        }
        let background: HashMap<DocId, bool> = self
            .inner
            .docs
            .lock()
            .iter()
            .map(|(id, d)| (*id, d.state == DocState::Background))
            .collect();
        struct Candidate {
            foreground: bool,
            class: CacheClass,
            cache: Arc<dyn Evictable>,
            evicted: Arc<AtomicU64>,
        }
        let mut candidates: Vec<Candidate> = self
            .inner
            .regs
            .lock()
            .values()
            .filter(|r| scope.is_none() || r.doc == scope)
            .filter_map(|r| {
                let cache = r.target.as_ref()?.upgrade()?;
                let in_background = r.doc.is_some_and(|d| background.get(&d) == Some(&true));
                Some(Candidate {
                    foreground: !in_background,
                    class: r.class,
                    cache,
                    evicted: Arc::clone(&r.evicted),
                })
            })
            .collect();
        // Background before foreground, then cheapest class first.
        candidates.sort_by_key(|c| (c.foreground, c.class));

        for c in candidates {
            let now = usage();
            if now <= target {
                break;
            }
            let freed = c.cache.evict(now - target);
            c.evicted.fetch_add(freed, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> GovernorStats {
        let config = self.config();
        GovernorStats {
            total_bytes: self.total_bytes(),
            global_budget: config.global,
            per_document_budget: config.per_document,
            caches: self
                .inner
                .regs
                .lock()
                .values()
                .map(|r| CacheStats {
                    name: r.name,
                    doc: r.doc,
                    class: r.class,
                    bytes: clamp(r.bytes.load(Ordering::Acquire)),
                    evicted_bytes: r.evicted.load(Ordering::Relaxed),
                })
                .collect(),
        }
    }
}

impl Registration {
    /// Records a size change (positive when the cache grew). May evict, so
    /// never call it while holding the cache's own lock.
    pub fn charge(&self, delta: i64) {
        if delta == 0 {
            return;
        }
        self.bytes.fetch_add(delta, Ordering::AcqRel);
        if let Some(doc) = &self.doc_bytes {
            doc.fetch_add(delta, Ordering::AcqRel);
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.total.fetch_add(delta, Ordering::AcqRel);
        if delta > 0 {
            let config = inner.config.lock().clone();
            let doc_over = self
                .doc_bytes
                .as_ref()
                .is_some_and(|d| clamp(d.load(Ordering::Acquire)) > config.per_document);
            let global_over = clamp(inner.total.load(Ordering::Acquire)) > config.global;
            if doc_over || global_over {
                MemoryGovernor { inner }.rebalance();
            }
        }
    }

    pub fn bytes(&self) -> u64 {
        clamp(self.bytes.load(Ordering::Acquire))
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let bytes = self.bytes.swap(0, Ordering::AcqRel);
        if let Some(doc) = &self.doc_bytes {
            doc.fetch_sub(bytes, Ordering::AcqRel);
        }
        if let Some(inner) = self.inner.upgrade() {
            inner.total.fetch_sub(bytes, Ordering::AcqRel);
            inner.regs.lock().remove(&self.id);
        }
    }
}

/// A size-bounded LRU cache that registers with the governor.
pub struct LruCache<K, V> {
    state: Mutex<LruState<K, V>>,
    registration: Registration,
}

struct LruState<K, V> {
    entries: HashMap<K, (V, u64, u64)>,
    /// Recency tick to key, oldest first.
    order: BTreeMap<u64, K>,
    tick: u64,
}

impl<K, V> fmt::Debug for LruCache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LruCache")
            .field("bytes", &self.registration.bytes())
            .finish()
    }
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(
        governor: &MemoryGovernor,
        name: &'static str,
        doc: Option<DocId>,
        class: CacheClass,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let weak: Weak<dyn Evictable> = weak.clone();
            Self {
                state: Mutex::new(LruState {
                    entries: HashMap::new(),
                    order: BTreeMap::new(),
                    tick: 0,
                }),
                registration: governor.register(name, doc, class, weak),
            }
        })
    }

    /// Looks up a value and marks it most recently used.
    pub fn get(&self, key: &K) -> Option<V> {
        let mut st = self.state.lock();
        st.tick += 1;
        let tick = st.tick;
        let (value, old_tick) = {
            let (value, _, entry_tick) = st.entries.get_mut(key)?;
            let old = std::mem::replace(entry_tick, tick);
            (value.clone(), old)
        };
        st.order.remove(&old_tick);
        st.order.insert(tick, key.clone());
        Some(value)
    }

    /// Inserts or replaces a value of the given size in bytes.
    pub fn insert(&self, key: K, value: V, size: u64) {
        let delta = {
            let mut st = self.state.lock();
            st.tick += 1;
            let tick = st.tick;
            let old = st.entries.insert(key.clone(), (value, size, tick));
            st.order.insert(tick, key);
            match old {
                Some((_, old_size, old_tick)) => {
                    st.order.remove(&old_tick);
                    size as i64 - old_size as i64
                }
                None => size as i64,
            }
        };
        self.registration.charge(delta);
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        let (value, size) = {
            let mut st = self.state.lock();
            let (value, size, tick) = st.entries.remove(key)?;
            st.order.remove(&tick);
            (value, size)
        };
        self.registration.charge(-(size as i64));
        Some(value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.state.lock().entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.state.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> u64 {
        self.registration.bytes()
    }
}

impl<K, V> Evictable for LruCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn evict(&self, bytes: u64) -> u64 {
        let freed = {
            let mut st = self.state.lock();
            let mut freed = 0;
            while freed < bytes {
                let Some((_, key)) = st.order.pop_first() else {
                    break;
                };
                if let Some((_, size, _)) = st.entries.remove(&key) {
                    freed += size;
                }
            }
            freed
        };
        self.registration.charge(-(freed as i64));
        freed
    }
}

#[cfg(test)]
mod tests;
