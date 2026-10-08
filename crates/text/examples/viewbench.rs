//! Times the core side of a view request on a large file:
//! `cargo run -p quire-text --release --example viewbench -- <file>`.

use std::time::Instant;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::document::Action;
use quire_text::file::{OpenSettings, open};
use quire_text::layout::Layout;
use quire_text::view::{ViewRequest, build};

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let g = MemoryGovernor::new(GovernorConfig::default());
    let mut f = open(
        path.as_ref(),
        &g,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: std::env::temp_dir(),
            threads: 4,
            progress: &|_| true,
        },
    )
    .unwrap();
    let t = Instant::now();
    let e = f.original_index();
    e.source.prefetch(0, e.source.len());
    e.index
        .scan_range(&*e.source, 0, e.index.chunk_count(), None)
        .unwrap();
    f.doc.buffer_mut().refresh_index();
    println!("index {:?}", t.elapsed());
    let len = f.doc.len();
    for wrap in [None, Some(120)] {
        let mut times = Vec::new();
        for i in 0..50u64 {
            let anchor = (len / 50) * i + 12345;
            let req = ViewRequest {
                anchor,
                caret: false,
                before: 70,
                after: 105,
                layout: Layout { wrap, tab: 4 },
                hcols: Some((0, 240)),
            };
            let t = Instant::now();
            let msg = build(&f.doc, &req, &|_, _| Vec::new()).unwrap();
            times.push((t.elapsed().as_secs_f64() * 1000.0, msg.len()));
        }
        times.sort_by(|a, b| a.0.total_cmp(&b.0));
        println!(
            "wrap {wrap:?}: build p50 {:.2} ms, max {:.2} ms, {} bytes",
            times[25].0, times[49].0, times[25].1
        );
    }
    // Typing: apply + build.
    f.doc
        .apply(&Action::SetSelections {
            ranges: vec![(len / 2, len / 2)],
        })
        .unwrap();
    let mut times = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        f.doc.apply(&Action::Type { text: "a".into() }).unwrap();
        let a = t.elapsed();
        let req = ViewRequest {
            anchor: 0,
            caret: true,
            before: 70,
            after: 105,
            layout: Layout::default(),
            hcols: Some((0, 240)),
        };
        build(&f.doc, &req, &|_, _| Vec::new()).unwrap();
        times.push((a.as_secs_f64() * 1000.0, t.elapsed().as_secs_f64() * 1000.0));
    }
    times.sort_by(|a, b| a.1.total_cmp(&b.1));
    println!(
        "type apply p50 {:.3} ms; apply+build p50 {:.2} ms, max {:.2} ms",
        times[50].0, times[50].1, times[99].1
    );
}
