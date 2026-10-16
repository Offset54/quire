//! Markdown engine costs (M2 task 13; ADR-0027): building the block index of
//! a whole document, re-parsing after an edit, and rendering one block.
//! Ref-sim and the in-app scenarios measure the same work end to end; these
//! guard the engine alone against regressions.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use quire_markdown::document::MarkdownDoc;
use quire_markdown::index::BlockIndex;
use quire_markdown::render::RenderOptions;

/// One section of mixed CommonMark + GFM (headings, lists, a table, code,
/// math, a quote, reference links and footnotes).
fn section(i: usize) -> String {
    format!(
        "## Section {i}\n\nA paragraph with *emphasis*, `code`, a [link][r{i}] and a note[^n{i}].\n\
         It goes on for a second line with 한국어 and 日本語.\n\n\
         - item one\n- item two\n  - nested [ ] task\n\n\
         | a | b | c |\n|---|:-:|--:|\n| 1 | two | 3 |\n| 4 | five | 6 |\n\n\
         ```rust\nfn f{i}(x: u32) -> u32 {{\n    x * 2\n}}\n```\n\n\
         > A quote\n> > nested\n\n$$\nx_{i}^2\n$$\n\n\
         [r{i}]: https://example.com/{i}\n[^n{i}]: Note {i}.\n\n"
    )
}

/// About `mib` MiB of sections.
fn document(mib: usize) -> Vec<u8> {
    let mut out = String::with_capacity(mib << 20);
    let mut i = 0;
    while out.len() < mib << 20 {
        out.push_str(&section(i));
        i += 1;
    }
    out.into_bytes()
}

fn full_doc(text: &[u8]) -> MarkdownDoc {
    let mut d = MarkdownDoc::new(text.len() as u64);
    while !d.index().is_complete() {
        d.extend(text).unwrap();
    }
    d
}

fn build(c: &mut Criterion) {
    let text = document(4);
    let mut g = c.benchmark_group("index");
    g.sample_size(10);
    g.bench_function("build_4mib", |b| {
        b.iter(|| {
            let mut ix = BlockIndex::new(text.len() as u64);
            while !ix.is_complete() {
                ix.extend(text.as_slice()).unwrap();
            }
            black_box(ix.blocks().len())
        });
    });
    g.bench_function("document_4mib", |b| {
        b.iter(|| black_box(full_doc(&text).index().blocks().len()));
    });
    g.finish();
}

/// One keystroke and its undo, at `at`, through the document (index,
/// analyses and table invalidation).
fn edit(c: &mut Criterion) {
    let text = document(4);
    let mid = text.len() / 2;
    // The start of a paragraph line near the middle.
    let para = mid
        + text[mid..]
            .windows(13)
            .position(|w| w == b"A paragraph w")
            .unwrap();
    // The line of a closing code fence near the middle: deleting a backtick
    // re-opens the fence and widens the re-parse to the edit budget.
    let fence = mid + text[mid..].windows(5).position(|w| w == b"}\n```").unwrap() + 2;
    let cases = [("paragraph", para, b'x'), ("fence", fence, b'`')];
    let mut g = c.benchmark_group("edit");
    for (name, at, ch) in cases {
        // Each iteration restores the text, so one document serves them all.
        let mut t = text.clone();
        let mut d = full_doc(&t);
        g.bench_function(format!("type_and_undo/{name}"), |b| {
            b.iter(|| {
                if ch == b'`' {
                    // Delete one backtick of the fence, then put it back.
                    t.remove(at);
                    d.edit(t.as_slice(), at as u64, 1, 0).unwrap();
                    t.insert(at, ch);
                    black_box(d.edit(t.as_slice(), at as u64, 0, 1).unwrap())
                } else {
                    t.insert(at, ch);
                    d.edit(t.as_slice(), at as u64, 0, 1).unwrap();
                    t.remove(at);
                    black_box(d.edit(t.as_slice(), at as u64, 1, 0).unwrap())
                }
            });
        });
    }
    g.finish();
}

fn render(c: &mut Criterion) {
    let text = document(1);
    let mut d = full_doc(&text);
    let opts = RenderOptions::default();
    let n = d.index().blocks().len();
    let mut g = c.benchmark_group("render");
    g.bench_function("one_block", |b| {
        let mut i = n / 2;
        b.iter(|| {
            i = if i + 1 >= n { n / 2 } else { i + 1 };
            black_box(d.render(text.as_slice(), i, &opts).unwrap())
        });
    });
    g.finish();
}

criterion_group!(benches, build, edit, render);
criterion_main!(benches);
