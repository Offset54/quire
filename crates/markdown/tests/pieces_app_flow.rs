//! The preview's pieces of a large table through the text engine, as the
//! app drives them: text typed key by key at the top (changes merged as the
//! host merges them), then a window at the table.

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_markdown::document::MarkdownDoc;
use quire_markdown::preview::Preview;
use quire_markdown::render::RenderOptions;
use quire_text::document::{Action, TextChange};
use quire_text::file::{OpenSettings, open};

fn sample() -> String {
    let mut parts = vec![
        "# Notes title\n".to_owned(),
        "First paragraph with **bold** text.\n".to_owned(),
    ];
    for i in 0..400 {
        parts.push(format!("## Section {i}\n"));
        parts.push(format!(
            "Paragraph {i} with some words to fill the line, and more words after them.\n"
        ));
    }
    let mut rows = vec!["| Name | Value | Note |\n|---|--:|---|".to_owned()];
    for i in 0..300 {
        rows.push(format!("| row {i} | {} | 한국어 {} |", i * 3, i % 7));
    }
    parts.push(rows.join("\n") + "\n");
    parts.push("[far]: https://example.com/far\n".to_owned());
    parts.join("\n")
}

#[test]
fn pieces_render_the_table_after_typing_above_it() {
    let dir = std::env::temp_dir().join(format!("quire-pieces-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.md");
    std::fs::write(&path, sample()).unwrap();
    let governor = MemoryGovernor::new(GovernorConfig::default());
    let mut file = open(
        &path,
        &governor,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: std::env::temp_dir(),
            threads: 2,
            progress: &|_| true,
        },
    )
    .unwrap();
    let mut doc = MarkdownDoc::new(file.doc.len());
    let mut preview = Preview::new(RenderOptions::default());
    let snap = file.doc.snapshot();
    preview
        .window(&mut doc, &snap, 0, 40, 80, &|_, _| false)
        .unwrap();
    while !doc.index().is_complete() {
        doc.extend(&snap).unwrap();
    }
    file.doc.take_change();
    // Type "Hello " at the start, merging changes in pairs as the host's
    // pending change does when the worker is behind.
    let mut pending: Option<TextChange> = None;
    for (k, ch) in "Hello ".chars().enumerate() {
        file.doc
            .apply(&Action::SetSelections {
                ranges: vec![(k as u64, k as u64)],
            })
            .unwrap();
        file.doc.take_change();
        file.doc
            .apply(&Action::Type {
                text: ch.to_string(),
            })
            .unwrap();
        let c = file.doc.take_change().expect("a change");
        pending = Some(match pending {
            Some(p) => p.then(c),
            None => c,
        });
        if k % 2 == 1 {
            let c = pending.take().unwrap();
            doc.edit(&file.doc.snapshot(), c.from, c.removed, c.inserted)
                .unwrap();
        }
    }
    let snap = file.doc.snapshot();
    let text = String::from_utf8(snap.read(0..snap.len()).unwrap()).unwrap();
    assert!(text.starts_with("Hello # Notes"));
    for b in doc.index().blocks() {
        let s = &text.as_bytes()[b.start as usize..b.end as usize];
        if b.kind == quire_markdown::scan::BlockKind::Table {
            assert!(s.starts_with(b"| Name |"), "table block at {}", b.start);
        }
    }
    let at = text.find("| row 150 |").unwrap() as u64;
    let w = preview
        .window(&mut doc, &snap, at, 40, 80, &|_, _| false)
        .unwrap();
    let a = &w.blocks[w.anchor];
    let h = &a.html.as_ref().unwrap().html;
    assert!(h.contains("<table") && h.contains("row 150"), "{h}");
    for e in &w.blocks {
        let h = &e.html.as_ref().unwrap().html;
        if e.id & quire_markdown::pieces::PIECE_ID_BIT != 0 {
            assert!(
                h.contains("<table"),
                "piece at {}: {}",
                e.start,
                &h[..h.len().min(200)]
            );
        }
    }
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
}
