//! Block index timings on a real file through the text engine (mapped source,
//! piece tree snapshot): the first stretch (what first paint needs), the
//! whole index (background), and edits at several positions.
//!
//! `cargo run --release -p quire-markdown --example indexbench -- <file.md>`

use std::path::PathBuf;
use std::time::Instant;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_markdown::index::{BlockIndex, EDIT_BUDGET};
use quire_text::document::Action;
use quire_text::file::{OpenSettings, open};

fn main() -> std::io::Result<()> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: indexbench <file.md>"),
    );
    let governor = MemoryGovernor::new(GovernorConfig::default());
    let t = Instant::now();
    let mut file = open(
        &path,
        &governor,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: std::env::temp_dir(),
            threads: 4,
            progress: &|_| true,
        },
    )?;
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let snap = file.doc.snapshot();
    let len = snap.len();

    let mut ix = BlockIndex::new(len);
    let t = Instant::now();
    let first = ix.extend(&snap)?;
    let first_ms = t.elapsed().as_secs_f64() * 1e3;
    let first_blocks = first.len();
    let first_frontier = ix.frontier();

    let t = Instant::now();
    let mut stretches = 1;
    let mut worst = 0f64;
    while !ix.is_complete() {
        let s = Instant::now();
        ix.extend(&snap)?;
        worst = worst.max(s.elapsed().as_secs_f64() * 1e3);
        stretches += 1;
    }
    let full_ms = first_ms + t.elapsed().as_secs_f64() * 1e3;
    println!(
        "open {open_ms:.1} ms; first stretch {first_ms:.2} ms ({first_blocks} blocks, {first_frontier} bytes); \
         whole index {full_ms:.0} ms ({} blocks, {stretches} stretches, worst stretch {worst:.2} ms)",
        ix.blocks().len()
    );

    // Typing one character at a few positions: time of the index update.
    for frac in [0.0, 0.25, 0.5, 0.75, 0.999] {
        let b = ix.blocks()[((ix.blocks().len() - 1) as f64 * frac) as usize].clone();
        let at = b.start + (b.end - b.start) / 2;
        file.doc.apply(&Action::SetSelections {
            ranges: vec![(at, at)],
        })?;
        let at = file.doc.primary().head;
        file.doc.apply(&Action::Type { text: "x".into() })?;
        let snap = file.doc.snapshot();
        let t = Instant::now();
        let ch = ix.edit(&snap, at, 0, 1, EDIT_BUDGET)?;
        println!(
            "edit at {:>5.1} % ({:?}): {:.3} ms, scanned {} bytes, {} block(s) replaced",
            frac * 100.0,
            b.kind,
            t.elapsed().as_secs_f64() * 1e3,
            ch.scanned,
            ch.inserted
        );
    }
    Ok(())
}
