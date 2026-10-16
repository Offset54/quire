use proptest::prelude::*;

use super::*;
use crate::scan::scan_document;

/// (start, end, kind) of every block of a whole-document scan.
fn truth(text: &[u8]) -> Vec<(u64, u64, BlockKind)> {
    scan_document(text, &Flavor::PRODUCT)
        .into_iter()
        .map(|f| (f.range.start as u64, f.range.end as u64, f.kind))
        .collect()
}

fn shape(index: &BlockIndex) -> Vec<(u64, u64, BlockKind)> {
    index
        .blocks()
        .iter()
        .map(|b| (b.start, b.end, b.kind.clone()))
        .collect()
}

fn full(text: &[u8], chunk: usize) -> BlockIndex {
    let mut ix = BlockIndex::with_chunk(text.len() as u64, chunk);
    complete(&mut ix, text);
    ix
}

fn complete(ix: &mut BlockIndex, text: &[u8]) {
    while !ix.is_complete() {
        ix.extend(text).unwrap();
    }
}

const DOC: &str = "---\ntitle: t\n---\n\n# One\n\nA paragraph\nwith two lines.\n\n- item\n- item two\n\n  more\n\n> quote\n> > nested\n\n```rust\nfn f() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n[r]: /x\n\nUses [r] and a note[^1].\n\n[^1]: The note.\n\n$$\nx^2\n$$\n\n<details>\n<summary>S</summary>\n\nhidden\n\n</details>\n\nEnd.\n";

#[test]
fn chunked_index_matches_whole_scan() {
    let t = DOC.as_bytes();
    for chunk in [16, 40, 100, 1000, 1 << 20] {
        assert_eq!(shape(&full(t, chunk)), truth(t), "chunk {chunk}");
    }
}

/// U+FEFF starting a line past the document's start is text, in every chunk
/// size (comrak skips it only at the start of what it parses; md_scan fuzz).
#[test]
fn a_byte_order_mark_mid_document_is_text() {
    let t = "\u{feff}# Title\n\npara\n\n\u{feff}> not a quote\n\n\u{feff}- not a list\n".as_bytes();
    let kinds: Vec<_> = truth(t).into_iter().map(|b| b.2).collect();
    assert_eq!(
        kinds,
        [
            BlockKind::Heading {
                level: 1,
                setext: false
            },
            BlockKind::Paragraph,
            BlockKind::Paragraph,
            BlockKind::Paragraph
        ]
    );
    for chunk in [8, 16, 40, 1 << 20] {
        assert_eq!(shape(&full(t, chunk)), truth(t), "chunk {chunk}");
    }
}

#[test]
fn hashes_follow_bytes() {
    let ix = full(DOC.as_bytes(), 64);
    for b in ix.blocks() {
        assert_eq!(
            b.hash,
            xxh3_64(&DOC.as_bytes()[b.start as usize..b.end as usize])
        );
    }
}

fn apply(text: &mut Vec<u8>, from: usize, removed: usize, ins: &[u8]) {
    text.splice(from..from + removed, ins.iter().copied());
}

#[test]
fn typing_in_a_paragraph_touches_two_blocks_and_keeps_ids() {
    let mut t = DOC.as_bytes().to_vec();
    let mut ix = full(&t, 64);
    let before: Vec<u64> = ix.blocks().iter().map(|b| b.id).collect();
    let at = DOC.find("with two").unwrap();
    apply(&mut t, at, 0, b"X");
    let ch = ix.edit(&t, at as u64, 0, 1, EDIT_BUDGET).unwrap();
    assert_eq!(shape(&ix), truth(&t));
    assert!(!ch.truncated);
    // Only the paragraph changed; the heading before it kept its id.
    assert_eq!(ch.removed.len(), 1);
    let after: Vec<u64> = ix.blocks().iter().map(|b| b.id).collect();
    let changed: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(changed.len(), 1, "{before:?} {after:?}");
    assert!(ch.scanned < 200, "scanned {}", ch.scanned);
}

#[test]
fn opening_a_fence_swallows_the_rest() {
    let mut t = DOC.as_bytes().to_vec();
    let mut ix = full(&t, 64);
    let at = DOC.find("A paragraph").unwrap();
    apply(&mut t, at, 0, b"```\n");
    let ch = ix.edit(&t, at as u64, 0, 4, EDIT_BUDGET).unwrap();
    complete(&mut ix, &t);
    assert_eq!(shape(&ix), truth(&t));
    assert!(!ch.truncated);
}

#[test]
fn a_small_budget_cuts_the_index_and_extending_repairs_it() {
    let mut t = DOC.repeat(20).into_bytes();
    let mut ix = full(&t, 64);
    // A tilde fence nothing closes runs to the end: nothing lines up again.
    let at = DOC.find("A paragraph").unwrap();
    apply(&mut t, at, 0, b"~~~\n");
    let ch = ix.edit(&t, at as u64, 0, 4, 256).unwrap();
    assert!(ch.truncated);
    assert!(ix.frontier() < t.len() as u64);
    complete(&mut ix, &t);
    assert_eq!(shape(&ix), truth(&t));
}

#[test]
fn setext_underline_joins_the_previous_paragraph() {
    let mut t = b"Title\n\nnext\n".to_vec();
    let mut ix = full(&t, 16);
    // Remove the blank line, then turn "next" into an underline.
    apply(&mut t, 6, 1, b"");
    ix.edit(&t, 6, 1, 0, EDIT_BUDGET).unwrap();
    assert_eq!(shape(&ix), truth(&t));
    apply(&mut t, 6, 4, b"===");
    ix.edit(&t, 6, 4, 3, EDIT_BUDGET).unwrap();
    assert_eq!(shape(&ix), truth(&t));
    assert!(matches!(
        ix.blocks()[0].kind,
        BlockKind::Heading { setext: true, .. }
    ));
}

#[test]
fn edits_beyond_the_frontier_leave_the_index_alone() {
    let mut t = DOC.repeat(4).into_bytes();
    let mut ix = BlockIndex::with_chunk(t.len() as u64, 64);
    ix.extend(&t).unwrap();
    let frontier = ix.frontier();
    let at = t.len() - 3;
    apply(&mut t, at, 0, b"zz");
    let ch = ix.edit(&t, at as u64, 0, 2, EDIT_BUDGET).unwrap();
    assert_eq!(ch.inserted, 0);
    assert_eq!(ix.frontier(), frontier);
    complete(&mut ix, &t);
    assert_eq!(shape(&ix), truth(&t));
}

/// Replays a proptest case: `(at, len, text)` with proptest's `Index` values.
fn replay(doc: &[u8], edits: &[(u64, u64, &str)], chunk: usize, budget: u64, trace: bool) {
    let idx = |v: u64, n: usize| ((v as u128 * n as u128) >> 64) as usize;
    let mut t = doc.to_vec();
    let mut ix = BlockIndex::with_chunk(t.len() as u64, chunk);
    complete(&mut ix, &t);
    for &(a, l, s) in edits {
        let at = idx(a, t.len() + 1);
        let removed = idx(l, (t.len() - at).min(20) + 1);
        apply(&mut t, at, removed, s.as_bytes());
        let ch = ix
            .edit(&t, at as u64, removed as u64, s.len() as u64, budget)
            .unwrap();
        if trace {
            eprintln!(
                "edit at {at} -{removed} +{s:?} => {:?}",
                String::from_utf8_lossy(&t)
            );
            eprintln!("  change {ch:?} frontier {}", ix.frontier());
            eprintln!("  index {:?}", shape(&ix));
            eprintln!("  truth {:?}", truth(&t));
        }
        let indexed: Vec<_> = truth(&t)
            .into_iter()
            .filter(|b| b.1 <= ix.frontier())
            .collect();
        assert_eq!(shape(&ix), indexed);
    }
}

#[test]
fn regression_definition_destination_on_the_next_line() {
    replay(
        b" -->text  \n===\n[x]: /u\n```\n```\n*e*\n\n***\n\n\n[x]: /u\n",
        &[
            (7234017283807667301, 1756832768924719202, ""),
            (12423317437396228640, 13835058055282163712, "\n```\n"),
            (9662580229085955609, 0, "\n\n"),
        ],
        16,
        32,
        false,
    );
}

#[test]
fn regression_front_matter_appears_at_the_start() {
    replay(
        b"| a | b |\ntext  \n> \n\ntext  \n---\npara\n\n\n\n| a | b |\n---\n```",
        &[
            (0, 7027331075698876807, "\r\n\\"),
            (17054536973806943947, 0, "\n\n\n```\n"),
            (3381903080180084463, 7905747460161236407, "[x]: /u\n"),
            (0, 5270498306774157605, "\\"),
            (0, 14933078535860113213, ""),
        ],
        16,
        32,
        false,
    );
}

/// Pieces random documents and edits are made of: every container and leaf
/// kind, and the characters that change block structure.
const PIECES: &[&str] = &[
    "para",
    "more text",
    "\n",
    "\n\n",
    "# ",
    "## h\n",
    "===\n",
    "---\n",
    "- ",
    "* ",
    "1. ",
    "  ",
    "    ",
    "> ",
    "```",
    "~~~",
    "\n```\n",
    "<div>",
    "</div>\n",
    "<!-- ",
    " -->",
    "| a | b |\n",
    "|---|---|\n",
    "[x]: /u\n",
    "[x]",
    "[^1]",
    "[^1]: n\n",
    "$$\n",
    "\t",
    "\r\n",
    "***\n",
    "- [ ] t\n",
    "<details>\n",
    "text  \n",
    "\\",
    "*e*",
    "`c`",
    "한글",
    "\u{0}",
];

fn doc_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(PIECES), 0..60).prop_map(|v| v.concat().into_bytes())
}

#[derive(Debug, Clone)]
struct Edit {
    at: prop::sample::Index,
    len: prop::sample::Index,
    text: String,
}

fn edit_strategy() -> impl Strategy<Value = Edit> {
    (
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        prop::collection::vec(prop::sample::select(PIECES), 0..3).prop_map(|v| v.concat()),
    )
        .prop_map(|(at, len, text)| Edit { at, len, text })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Incremental result == full parse after random edit sequences (the
    /// M2 plan's task 2 property), with chunked building, partial indexes
    /// and small edit budgets.
    #[test]
    fn incremental_equals_full(
        doc in doc_strategy(),
        edits in prop::collection::vec(edit_strategy(), 1..8),
        chunk in prop::sample::select(vec![16usize, 48, 200, 4096]),
        budget in prop::sample::select(vec![32u64, 300, EDIT_BUDGET]),
        partial in any::<bool>(),
    ) {
        let mut t = doc;
        let mut ix = BlockIndex::with_chunk(t.len() as u64, chunk);
        if partial {
            ix.extend(&t).unwrap();
        } else {
            complete(&mut ix, &t);
        }
        for e in edits {
            let at = e.at.index(t.len() + 1);
            let removed = e.len.index((t.len() - at).min(20) + 1);
            // Keep edits on character boundaries, as the editor does.
            let s = String::from_utf8_lossy(&t).into_owned();
            if !s.is_char_boundary(at) || !s.is_char_boundary(at + removed) || s.len() != t.len() {
                continue;
            }
            apply(&mut t, at, removed, e.text.as_bytes());
            let ch = ix.edit(&t, at as u64, removed as u64, e.text.len() as u64, budget).unwrap();
            prop_assert!(ch.first <= ix.blocks().len());
            // Whatever is indexed is right, before and after completing it.
            let indexed: Vec<_> = truth(&t).into_iter().filter(|b| b.1 <= ix.frontier()).collect();
            prop_assert_eq!(shape(&ix), indexed, "after edit at {} -{} +{:?}: {:?}", at, removed, e.text, String::from_utf8_lossy(&t));
        }
        complete(&mut ix, &t);
        prop_assert_eq!(shape(&ix), truth(&t), "completed: {:?}", String::from_utf8_lossy(&t));
        // Ids are unique.
        let mut ids: Vec<u64> = ix.blocks().iter().map(|b| b.id).collect();
        ids.sort_unstable();
        ids.dedup();
        prop_assert_eq!(ids.len(), ix.blocks().len());
    }
}
