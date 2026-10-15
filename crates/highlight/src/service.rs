//! Highlighting off the caller's thread (ADR-0029).
//!
//! Highlighting runs at about 1–3 MB/s, and building a grammar's
//! configuration on first use takes up to a few hundred milliseconds, so
//! the preview never waits for either while it holds its state. It asks
//! [`lookup`]: a result from the cache, or `None`, and then [`request`]s the
//! work, which one background thread does, newest request first. When a
//! result is ready, every registered listener is called (the preview's
//! worker renders the waiting blocks again). Results are cached by
//! (language, content) hash, up to [`CACHE_BYTES`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use xxhash_rust::xxh3::Xxh3;

use crate::{Plain, highlight};

/// Bytes of highlighted HTML kept.
pub const CACHE_BYTES: usize = 8 * 1024 * 1024;
/// Requests waiting at most; the oldest go first when more come.
const QUEUE_MAX: usize = 512;

/// A highlighting result: the HTML, or why the code stays plain.
pub type Highlighted = Result<Arc<str>, Plain>;

/// Called when results are ready.
pub type Listener = dyn Fn() + Send + Sync;

struct Job {
    key: u128,
    lang: String,
    code: String,
}

#[derive(Default)]
struct Cache {
    map: HashMap<u128, (Highlighted, u64)>,
    bytes: usize,
    tick: u64,
}

impl Cache {
    fn get(&mut self, key: u128) -> Option<Highlighted> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(&key).map(|(r, t)| {
            *t = tick;
            r.clone()
        })
    }

    fn insert(&mut self, key: u128, r: Highlighted) {
        self.tick += 1;
        let size = |r: &Highlighted| r.as_ref().map_or(0, |h| h.len()) + 48;
        self.bytes += size(&r);
        if let Some((old, _)) = self.map.insert(key, (r, self.tick)) {
            self.bytes -= size(&old);
        }
        if self.bytes > CACHE_BYTES {
            let mut by_age: Vec<(u64, u128)> =
                self.map.iter().map(|(k, (_, t))| (*t, *k)).collect();
            by_age.sort_unstable();
            for (_, k) in by_age {
                if self.bytes <= CACHE_BYTES * 3 / 4 {
                    break;
                }
                if let Some((old, _)) = self.map.remove(&k) {
                    self.bytes -= size(&old);
                }
            }
        }
    }
}

#[derive(Default)]
struct Shared {
    cache: Mutex<Cache>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    listeners: Mutex<Vec<Weak<Listener>>>,
    started: AtomicBool,
}

fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(Shared::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cache key of `code` in `lang`.
pub fn key(lang: &str, code: &str) -> u128 {
    let mut h = Xxh3::new();
    h.update(lang.to_ascii_lowercase().as_bytes());
    h.update(&[0]);
    h.update(code.as_bytes());
    h.digest128()
}

/// The cached result for `key`, if any.
pub fn lookup(key: u128) -> Option<Highlighted> {
    lock(&shared().cache).get(key)
}

/// Whether a result for `key` is cached (without counting as a use).
pub fn is_ready(key: u128) -> bool {
    lock(&shared().cache).map.contains_key(&key)
}

/// Highlights `code` now, on this thread, and caches the result.
pub fn compute(lang: &str, code: &str) -> Highlighted {
    let r = highlight(lang, code).map(Arc::from);
    lock(&shared().cache).insert(key(lang, code), r.clone());
    r
}

/// Asks the background thread to highlight `code`; listeners are called
/// when it is done.
pub fn request(lang: &str, code: &str) {
    let s = shared();
    if !s.started.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new()
            .name("quire-highlight".into())
            .spawn(run);
        if spawned.is_err() {
            // No thread: requests stay pending (the code shows plain).
            s.started.store(false, Ordering::Release);
            return;
        }
    }
    let k = key(lang, code);
    let mut q = lock(&s.queue);
    if q.iter().any(|j| j.key == k) {
        return;
    }
    if q.len() >= QUEUE_MAX {
        q.pop_front();
    }
    q.push_back(Job {
        key: k,
        lang: lang.to_owned(),
        code: code.to_owned(),
    });
    s.wake.notify_one();
}

/// Calls `l` whenever results are ready, while it lives.
pub fn add_listener(l: Weak<Listener>) {
    let mut ls = lock(&shared().listeners);
    ls.retain(|w| w.strong_count() > 0);
    ls.push(l);
}

fn run() {
    let s = shared();
    loop {
        let job = {
            let mut q = lock(&s.queue);
            loop {
                // Newest first: what is on screen now.
                if let Some(j) = q.pop_back() {
                    break j;
                }
                q = s.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        if lock(&s.cache).map.contains_key(&job.key) {
            continue;
        }
        let r = highlight(&job.lang, &job.code).map(Arc::from);
        lock(&s.cache).insert(job.key, r);
        let more = !lock(&s.queue).is_empty();
        // Batch notifications while work is queued, but never leave a
        // result unannounced for long: announce at least every 8 results.
        static DONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = DONE.fetch_add(1, Ordering::Relaxed) + 1;
        if !more || n.is_multiple_of(8) {
            let ls: Vec<Arc<Listener>> = lock(&s.listeners)
                .iter()
                .filter_map(Weak::upgrade)
                .collect();
            for l in ls {
                l();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn requests_are_done_in_the_background_and_announced() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let l: Arc<Listener> = Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        add_listener(Arc::downgrade(&l));
        let code = "fn service_test() -> u8 { 7 }\n";
        let k = key("rust", code);
        assert!(lookup(k).is_none());
        request("rust", code);
        request("Rust", code);
        let t0 = Instant::now();
        while !is_ready(k) {
            assert!(t0.elapsed() < Duration::from_secs(20), "not highlighted");
            std::thread::sleep(Duration::from_millis(5));
        }
        let html = lookup(k).unwrap().unwrap();
        assert!(html.contains("hl-keyword"));
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(t0.elapsed() < Duration::from_secs(20), "not announced");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(key("RUST", code), k);
    }

    #[test]
    fn failures_are_cached_as_plain() {
        let r = compute("cobol", "X.");
        assert_eq!(r, Err(Plain::Unknown));
        assert_eq!(lookup(key("cobol", "X.")), Some(Err(Plain::Unknown)));
    }

    #[test]
    fn the_cache_stays_within_its_bytes() {
        let mut c = Cache::default();
        let big: Arc<str> = Arc::from("x".repeat(1024 * 1024));
        for k in 0..20u128 {
            c.insert(k, Ok(Arc::clone(&big)));
        }
        assert!(c.bytes <= CACHE_BYTES);
        assert!(c.map.contains_key(&19));
        assert!(!c.map.contains_key(&0));
    }
}
