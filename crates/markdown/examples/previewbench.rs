//! Preview pipeline timings on a real file through the text engine: the
//! first window (what first paint needs from the core), the whole index with
//! analyses and tables (background), and typing at several positions: the
//! index update, the tables' rebuild and the window's re-render (the core's
//! part of keystroke → preview update).
//!
//! `cargo run --release -p quire-markdown --example previewbench -- <file.md>`

use std::path::PathBuf;
use std::time::Instant;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_markdown::document::MarkdownDoc;
use quire_markdown::preview::Preview;
use quire_markdown::render::RenderOptions;
use quire_text::document::Action;
use quire_text::file::{OpenSettings, open};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() -> std::io::Result<()> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: previewbench <file.md>"),
    );
    let governor = MemoryGovernor::new(GovernorConfig::default());
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
    let snap = file.doc.snapshot();
    let mut doc = MarkdownDoc::new(snap.len());
    let mut preview = Preview::new(RenderOptions::default());

    let t = Instant::now();
    let w = preview.window(&mut doc, &snap, 0, 40, 80, &|_, _| false)?;
    println!(
        "first window: {:.2} ms ({} blocks, {} bytes of HTML)",
        ms(t),
        w.blocks.len(),
        w.blocks
            .iter()
            .filter_map(|b| b.html.as_ref())
            .map(|r| r.html.len())
            .sum::<usize>()
    );

    let t = Instant::now();
    while !doc.index().is_complete() {
        doc.extend(&snap)?;
    }
    let index_ms = ms(t);
    let t = Instant::now();
    doc.tables();
    println!(
        "whole index with analyses: {index_ms:.0} ms ({} blocks); tables {:.2} ms ({} headings)",
        doc.index().blocks().len(),
        ms(t),
        doc.tables().outline().len()
    );

    let n = doc.index().blocks().len();
    for frac in [0.0, 0.25, 0.5, 0.75, 0.999] {
        let b = doc.index().blocks()[((n - 1) as f64 * frac) as usize].clone();
        let at = b.start + (b.end - b.start) / 2;
        // A window around the edit, as the preview shows it.
        preview.window(&mut doc, &file.doc.snapshot(), at, 40, 80, &|_, _| false)?;
        file.doc.apply(&Action::SetSelections {
            ranges: vec![(at, at)],
        })?;
        file.doc.take_change();
        file.doc.apply(&Action::Type { text: "x".into() })?;
        let c = file.doc.take_change().expect("a change");
        let snap = file.doc.snapshot();
        let t = Instant::now();
        let ch = doc.edit(&snap, c.from, c.removed, c.inserted)?;
        preview.forget(&ch.removed);
        let edit_ms = ms(t);
        let t = Instant::now();
        doc.tables();
        let tables_ms = ms(t);
        let t = Instant::now();
        let w = preview.window(&mut doc, &snap, at, 40, 80, &|_, _| true)?;
        let window_ms = ms(t);
        println!(
            "type at {:>5.1} % ({:?}): edit {edit_ms:.3} ms, tables {tables_ms:.2} ms, window {window_ms:.2} ms ({} blocks)",
            frac * 100.0,
            b.kind,
            w.blocks.len()
        );
    }

    // Edits that change what a block contributes: a heading's text, a new
    // footnote reference. The provisional window first, then the rebuild.
    let mid = doc.index().blocks()[n / 2].start;
    let heading = doc.index().block_at(mid);
    let heading = (heading..n)
        .find(|&i| {
            matches!(
                doc.index().blocks()[i].kind,
                quire_markdown::scan::BlockKind::Heading { .. }
            )
        })
        .expect("a heading");
    let para = (heading..n)
        .find(|&i| doc.index().blocks()[i].kind == quire_markdown::scan::BlockKind::Paragraph)
        .expect("a paragraph");
    // The biggest table, and the last block (the fixture's reference
    // definitions are there).
    let table = (0..n)
        .filter(|&i| doc.index().blocks()[i].kind == quire_markdown::scan::BlockKind::Table)
        .max_by_key(|&i| {
            let b = &doc.index().blocks()[i];
            b.end - b.start
        })
        .expect("a table");
    let tb = &doc.index().blocks()[table];
    println!("biggest table: {} bytes", tb.end - tb.start);
    let defs = n - 1;
    for (what, i, text) in [
        ("heading text", heading, "x"),
        ("new footnote reference", para, "[^0]"),
        ("inside the big table", table, "x"),
        ("a reference definition", defs, "x"),
    ] {
        let b = doc.index().blocks()[i].clone();
        let at = b.end - 2;
        preview.window(&mut doc, &file.doc.snapshot(), at, 40, 80, &|_, _| false)?;
        file.doc.apply(&Action::SetSelections {
            ranges: vec![(at, at)],
        })?;
        file.doc.take_change();
        file.doc.apply(&Action::Type { text: text.into() })?;
        let c = file.doc.take_change().expect("a change");
        let snap = file.doc.snapshot();
        let t = Instant::now();
        let ch = doc.edit(&snap, c.from, c.removed, c.inserted)?;
        preview.forget(&ch.removed);
        preview.set_provisional(true);
        preview.window(&mut doc, &snap, at, 40, 80, &|_, _| true)?;
        let first_ms = ms(t);
        let pending = doc.tables_pending();
        preview.set_provisional(false);
        let t = Instant::now();
        preview.window(&mut doc, &snap, at, 40, 80, &|_, _| true)?;
        println!(
            "{what}: edit and provisional window {first_ms:.2} ms (tables pending: {pending}); corrections {:.2} ms",
            ms(t)
        );
    }
    Ok(())
}
