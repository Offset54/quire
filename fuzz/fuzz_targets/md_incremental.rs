//! Incremental re-parsing equals a full parse (M2 task 2's property, as a
//! fuzz target, ADR-0027): after any sequence of edits, what the block index
//! has indexed matches a whole-document scan of the edited text, before and
//! after completing it, and block ids stay unique.
//!
//! Input: [flags][edit count n][n edits of 4 bytes: at hi, at lo, removed,
//! inserted length][document, then the inserted texts, taken in order].
//! Edits stay on character boundaries of the (lossily decoded) text, as the
//! editor's do.

#![no_main]

mod save_on_panic;

use libfuzzer_sys::fuzz_target;
use quire_markdown::index::{BlockIndex, EDIT_BUDGET};
use quire_markdown::options::Flavor;
use quire_markdown::scan::scan_document;

const CHUNKS: [usize; 4] = [16, 48, 200, 4096];
const BUDGETS: [u64; 3] = [32, 300, EDIT_BUDGET];

fn truth(text: &[u8]) -> Vec<(u64, u64, quire_markdown::scan::BlockKind)> {
    scan_document(text, &Flavor::PRODUCT)
        .into_iter()
        .map(|f| (f.range.start as u64, f.range.end as u64, f.kind))
        .collect()
}

fn shape(ix: &BlockIndex) -> Vec<(u64, u64, quire_markdown::scan::BlockKind)> {
    ix.blocks()
        .iter()
        .map(|b| (b.start, b.end, b.kind.clone()))
        .collect()
}

fn snap(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fuzz_target!(|data: &[u8]| {
    save_on_panic::guard("md_incremental", data);
    if data.len() < 2 {
        return;
    }
    let flags = data[0];
    let n = usize::from(data[1] % 8);
    let rest = &data[2..];
    if rest.len() < n * 4 {
        return;
    }
    let (edits, tail) = rest.split_at(n * 4);
    // The document and inserted texts as valid UTF-8, split on the last
    // edit's inserted lengths from the end.
    let mut tail = String::from_utf8_lossy(tail).into_owned();
    let mut inserts = Vec::new();
    for e in edits.chunks_exact(4).rev() {
        let len = usize::from(e[3] % 24);
        let at = snap(&tail, tail.len().saturating_sub(len));
        inserts.push(tail.split_off(at));
    }
    inserts.reverse();
    let mut t = tail.into_bytes();
    let chunk = CHUNKS[usize::from(flags) % CHUNKS.len()];
    let budget = BUDGETS[usize::from(flags >> 2) % BUDGETS.len()];
    let partial = flags & 0x40 != 0;

    let mut ix = BlockIndex::with_chunk(t.len() as u64, chunk);
    if partial {
        ix.extend(&t).unwrap();
    } else {
        while !ix.is_complete() {
            ix.extend(&t).unwrap();
        }
    }
    for (e, ins) in edits.chunks_exact(4).zip(&inserts) {
        let s = std::str::from_utf8(&t).expect("text stays UTF-8");
        let at = snap(
            s,
            (usize::from(e[0]) << 8 | usize::from(e[1])) % (t.len() + 1),
        );
        let end = snap(s, at + usize::from(e[2] % 21));
        let removed = end - at;
        t.splice(at..end, ins.bytes());
        let ch = ix
            .edit(&t, at as u64, removed as u64, ins.len() as u64, budget)
            .unwrap();
        assert!(ch.first <= ix.blocks().len());
        let indexed: Vec<_> = truth(&t)
            .into_iter()
            .filter(|b| b.1 <= ix.frontier())
            .collect();
        assert_eq!(
            shape(&ix),
            indexed,
            "after an edit at {at} -{removed} +{ins:?}"
        );
    }
    while !ix.is_complete() {
        ix.extend(&t).unwrap();
    }
    assert_eq!(shape(&ix), truth(&t), "completed");
    let mut ids: Vec<u64> = ix.blocks().iter().map(|b| b.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), ix.blocks().len(), "block ids repeat");
});
