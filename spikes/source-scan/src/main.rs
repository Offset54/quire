//! Spike for ADR-0007: how fast can a cold 2 GiB file be scanned (newline
//! count) through each access path?
//!
//! `source-scan <file> <mmap|mmap-prefetch|pread> <threads>` prints one JSON
//! line. Threads take 1 MiB chunks from a shared counter, so the file is
//! read roughly front to back whatever the thread count.

use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::source::{self, MappedSource, Source};

const CHUNK: u64 = 1 << 20;
const PREFETCH_AHEAD: u64 = 64 << 20;

fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) {
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, buf, offset).unwrap();
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, buf, offset).unwrap();
        assert!(n > 0);
        offset += n as u64;
        buf = &mut buf[n..];
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, variant, threads) = (&args[1], args[2].as_str(), args[3].parse::<u64>().unwrap());
    let file = File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    let mapped: Option<Arc<dyn Source>> = match variant {
        "mmap" | "mmap-prefetch" => Some(Arc::new(
            MappedSource::new(file.try_clone().unwrap()).unwrap(),
        )),
        "pread" => None,
        "open" => {
            let g = MemoryGovernor::new(GovernorConfig::default());
            let o = source::open(path.as_ref(), &g, None).unwrap();
            println!(
                "{}",
                serde_json::json!({"kind": format!("{:?}", o.kind), "remote": format!("{:?}", o.remote)})
            );
            return;
        }
        _ => panic!("unknown variant"),
    };
    let file = Arc::new(file);
    let next = Arc::new(AtomicU64::new(0));
    let prefetched = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let (mapped, file, next, prefetched) = (
                mapped.clone(),
                Arc::clone(&file),
                Arc::clone(&next),
                Arc::clone(&prefetched),
            );
            let prefetch = variant == "mmap-prefetch";
            std::thread::spawn(move || {
                let mut buf = vec![0u8; CHUNK as usize];
                let mut lines = 0u64;
                loop {
                    let at = next.fetch_add(CHUNK, Ordering::Relaxed);
                    if at >= len {
                        break;
                    }
                    let n = CHUNK.min(len - at) as usize;
                    match &mapped {
                        Some(m) => {
                            if prefetch {
                                let want = (at + PREFETCH_AHEAD).min(len);
                                let done = prefetched.load(Ordering::Relaxed);
                                if want > done
                                    && prefetched
                                        .compare_exchange(
                                            done,
                                            want,
                                            Ordering::Relaxed,
                                            Ordering::Relaxed,
                                        )
                                        .is_ok()
                                {
                                    m.prefetch(done, want - done);
                                }
                            }
                            m.read_at(at, &mut buf[..n]).unwrap();
                        }
                        None => read_exact_at(&file, at, &mut buf[..n]),
                    }
                    lines += memchr::memchr_iter(b'\n', &buf[..n]).count() as u64;
                }
                lines
            })
        })
        .collect();
    let lines: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{}",
        serde_json::json!({
            "variant": variant, "threads": threads, "bytes": len, "lines": lines,
            "seconds": secs, "mbPerSecond": len as f64 / 1e6 / secs
        })
    );
}
