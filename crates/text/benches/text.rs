//! Text engine costs (M1 tasks 4 and 14; ADR-0008): edits and lookups in a
//! fragmented piece tree, and the per-row work of building a view.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use quire_text::buffer::TextBuffer;
use quire_text::chars::display_text;
use quire_text::layout::{Layout, rows_around};

/// Deterministic pseudo-random numbers (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// About `mib` MiB of mixed-length lines.
fn text(mib: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(mib << 20);
    let words = [
        "piece",
        "tree",
        "편집기",
        "文書",
        "naïve",
        "index",
        "🙂",
        "line",
    ];
    let mut r = Rng(7);
    let mut line = 0u64;
    while out.len() < mib << 20 {
        let n = 1 + r.below(30);
        out.extend_from_slice(format!("{line:>9}|").as_bytes());
        for _ in 0..n {
            out.push(b' ');
            out.extend_from_slice(words[r.below(words.len() as u64) as usize].as_bytes());
        }
        out.push(b'\n');
        line += 1;
    }
    out
}

/// A document fragmented into about `edits` extra pieces.
fn fragmented(mib: usize, edits: usize) -> TextBuffer {
    let mut b = TextBuffer::from_bytes(&text(mib));
    let mut r = Rng(42);
    for _ in 0..edits {
        let at = r.below(b.len());
        b.replace(at, at, b"x").unwrap();
    }
    b
}

fn tree(c: &mut Criterion) {
    let mut b = fragmented(8, 100_000);
    let pieces = b.tree().piece_count();
    let lines = b.snapshot().line_count().unwrap();
    let mut r = Rng(1);
    c.bench_function(&format!("tree/insert_1_byte/{pieces}_pieces"), |bn| {
        bn.iter(|| {
            let at = r.below(b.len());
            b.replace(at, at, b"y").unwrap()
        });
    });
    let mut r = Rng(2);
    c.bench_function(&format!("tree/delete_1_byte/{pieces}_pieces"), |bn| {
        bn.iter(|| {
            let at = r.below(b.len() - 1);
            b.replace(at, at + 1, b"").unwrap()
        });
    });
    let snap = b.snapshot();
    let mut r = Rng(3);
    c.bench_function(&format!("tree/line_start/{pieces}_pieces"), |bn| {
        bn.iter(|| snap.line_start(black_box(r.below(lines))));
    });
    let mut r = Rng(4);
    c.bench_function(&format!("tree/line_of/{pieces}_pieces"), |bn| {
        bn.iter(|| snap.line_of(black_box(r.below(snap.len()))));
    });
    c.bench_function("tree/snapshot", |bn| bn.iter(|| black_box(b.snapshot())));
}

fn view(c: &mut Criterion) {
    // A long row (the unwrapped-line limit is 8 KiB) through the per-character
    // helpers that were once quadratic.
    let row: Vec<u8> = "abc 한글 déjà 🙂 ".repeat(8 * 1024 / 22).into_bytes();
    c.bench_function("view/display_text_8k_row", |bn| {
        bn.iter(|| display_text(black_box(&row)))
    });
    let b = TextBuffer::from_bytes(&text(8));
    let snap = b.snapshot();
    let mut r = Rng(5);
    for wrap in [None, Some(120)] {
        let layout = Layout { wrap, tab: 4 };
        let name = format!("view/rows_around_175/wrap_{}", wrap.unwrap_or(0));
        c.bench_function(&name, |bn| {
            bn.iter(|| rows_around(&snap, r.below(snap.len()), 70, 105, &layout).unwrap())
        });
    }
}

criterion_group!(benches, tree, view);
criterion_main!(benches);
