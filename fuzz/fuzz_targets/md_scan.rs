//! The Markdown block scanner (M2 task 12, ADR-0027): for any bytes, the
//! whole-document scan gives ordered, non-overlapping blocks inside the
//! document, and a block index built in chunks of any size gives the same
//! blocks as that one scan.
//!
//! Input: [chunk selector][document...].

#![no_main]

mod save_on_panic;

use libfuzzer_sys::fuzz_target;
use quire_markdown::index::BlockIndex;
use quire_markdown::options::Flavor;
use quire_markdown::scan::scan_document;

const CHUNKS: [usize; 4] = [16, 48, 200, 4096];

fuzz_target!(|data: &[u8]| {
    save_on_panic::guard("md_scan", data);
    let Some((&sel, doc)) = data.split_first() else {
        return;
    };
    let found = scan_document(doc, &Flavor::PRODUCT);
    let mut at = 0;
    for f in &found {
        assert!(f.range.start >= at, "blocks overlap or are out of order");
        assert!(f.range.start < f.range.end, "empty block");
        assert!(f.range.end <= doc.len(), "block past the end");
        at = f.range.end;
    }
    let truth: Vec<_> = found
        .into_iter()
        .map(|f| (f.range.start as u64, f.range.end as u64, f.kind))
        .collect();
    let mut ix = BlockIndex::with_chunk(doc.len() as u64, CHUNKS[usize::from(sel) % CHUNKS.len()]);
    while !ix.is_complete() {
        ix.extend(doc).unwrap();
    }
    let shape: Vec<_> = ix
        .blocks()
        .iter()
        .map(|b| (b.start, b.end, b.kind.clone()))
        .collect();
    assert_eq!(shape, truth);
});
