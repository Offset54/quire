//! Worker-pool measurements for ADR-0036, raw (not ref-sim).
//!
//! ```text
//! cargo run --release -p quire-pdf --example poolbench -- <pdfium lib> <file.pdf> [worker program [args...]]
//! ```
//!
//! Without a worker program it uses `quire-pdf-worker` next to this
//! example's directory. Prints one JSON object.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quire_pdf::protocol::{Matrix, flags};
use quire_pdf::{Pool, PoolConfig, WorkerCommand};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let library = PathBuf::from(&args[1]);
    let path = args[2].clone();
    let (program, wargs) = match args.get(3) {
        Some(p) => (PathBuf::from(p), args[4..].iter().map(Into::into).collect()),
        None => {
            let exe = std::env::current_exe().unwrap();
            let dir = exe.parent().unwrap().parent().unwrap();
            (
                dir.join(format!("quire-pdf-worker{}", std::env::consts::EXE_SUFFIX)),
                Vec::new(),
            )
        }
    };
    let make = |workers| {
        Pool::new(PoolConfig {
            workers,
            command: WorkerCommand {
                program: program.clone(),
                args: wargs.clone(),
                library: library.clone(),
                faults: false,
            },
            timeout: Duration::from_secs(60),
        })
    };

    // Spawn + handshake + open, five fresh pools.
    let mut open = Vec::new();
    for _ in 0..5 {
        let pool = make(1);
        let t = Instant::now();
        let (_doc, _) = pool.open_document(&path, None).unwrap();
        open.push(ms(t));
    }

    let pool = make(1);
    let (doc, info) = pool.open_document(&path, None).unwrap();
    let (w, h) = pool.page_sizes(doc, 0, 1).unwrap()[0];

    // Page 1 at 100 % (2 px per point) as one bitmap, and as 512 px tiles.
    let full_w = (w * 2.0).ceil() as u32;
    let full_h = (h * 2.0).ceil() as u32;
    let mut full = Vec::new();
    for _ in 0..7 {
        let t = Instant::now();
        pool.render(
            doc,
            0,
            Matrix::scale_translate(2.0, 0.0, 0.0),
            full_w,
            full_h,
            flags::ANNOT,
        )
        .unwrap();
        full.push(ms(t));
    }

    // 512 px tiles at 800 %, round trip per tile, one worker.
    let mut tile = Vec::new();
    for i in 0..21 {
        let t = Instant::now();
        let (x, y) = (w * 8.0 + (i % 3) as f32 * 512.0, h * 8.0);
        pool.render(
            doc,
            0,
            Matrix::scale_translate(16.0, x, y),
            512,
            512,
            flags::ANNOT,
        )
        .unwrap();
        tile.push(ms(t));
    }
    // A tiny tile shows the per-request overhead without pixels.
    let mut tiny = Vec::new();
    for _ in 0..21 {
        let t = Instant::now();
        pool.render(doc, 0, Matrix::scale_translate(16.0, 0.0, 0.0), 1, 1, 0)
            .unwrap();
        tiny.push(ms(t));
    }

    // A viewport at 800 % (1920 × 1080 device px: 4 × 3 tiles of 512),
    // across N workers in parallel.
    let mut viewport = Vec::new();
    for workers in [1usize, 3] {
        let pool = Arc::new(make(workers));
        let (doc, _) = pool.open_document(&path, None).unwrap();
        // Warm every worker on the document first.
        let warm: Vec<_> = (0..workers)
            .map(|_| {
                let pool = Arc::clone(&pool);
                std::thread::spawn(move || {
                    pool.render(doc, 1, Matrix::scale_translate(2.0, 0.0, 0.0), 64, 64, 0)
                })
            })
            .collect();
        for h in warm {
            h.join().unwrap().unwrap();
        }
        let mut runs = Vec::new();
        for page in [2u32, 3, 4, 5, 6] {
            let t = Instant::now();
            let jobs: Vec<_> = (0..12)
                .map(|k| {
                    let pool = Arc::clone(&pool);
                    let (x, y) = (
                        w * 4.0 + (k % 4) as f32 * 512.0,
                        h * 4.0 + (k / 4) as f32 * 512.0,
                    );
                    std::thread::spawn(move || {
                        pool.render(
                            doc,
                            page.min(info.pages - 1),
                            Matrix::scale_translate(16.0, x, y),
                            512,
                            512,
                            flags::ANNOT,
                        )
                    })
                })
                .collect();
            for j in jobs {
                j.join().unwrap().unwrap();
            }
            runs.push(ms(t));
        }
        viewport.push(serde_like(workers, &runs));
    }

    println!(
        "{{\"program\":{:?},\"file\":{:?},\"pages\":{},\"open_ms\":{:?},\"open_median_ms\":{:.1},\"page100_full_ms\":{:?},\"page100_full_median_ms\":{:.1},\"tile800_median_ms\":{:.2},\"tile800_ms\":{:?},\"tiny_median_ms\":{:.3},\"viewport800\":[{}],\"private_bytes\":{}}}",
        program.display().to_string(),
        path,
        info.pages,
        round(&open),
        median(open.clone()),
        round(&full),
        median(full.clone()),
        median(tile.clone()),
        round(&tile),
        median(tiny),
        viewport.join(","),
        pool.stats().private_bytes
    );
}

fn round(v: &[f64]) -> Vec<f64> {
    v.iter().map(|x| (x * 10.0).round() / 10.0).collect()
}

fn serde_like(workers: usize, runs: &[f64]) -> String {
    format!("{{\"workers\":{workers},\"ms\":{:?}}}", round(runs))
}
