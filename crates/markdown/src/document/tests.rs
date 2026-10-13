use proptest::prelude::*;

use super::*;

/// The document-wide tables of `text` built from scratch.
fn fresh(text: &[u8]) -> String {
    let mut d = MarkdownDoc::new(text.len() as u64);
    d.extend_to(text, text.len() as u64).unwrap();
    let blocks = d.index().blocks().to_vec();
    d.tables().summary(&blocks)
}

fn incremental(d: &mut MarkdownDoc) -> String {
    let blocks = d.index().blocks().to_vec();
    d.tables().summary(&blocks)
}

fn complete(d: &mut MarkdownDoc, text: &[u8]) {
    while !d.index().is_complete() {
        d.extend(text).unwrap();
    }
}

const DOC: &str = "# Title\n\nSee [x] and a note[^a].\n\n## Part\n\nMore text[^b], again[^a].\n\n# Title\n\n[x]: /one \"T\"\n\n[^a]: Note A.\n\n[^b]: Note B.\n";

#[test]
fn typing_in_a_paragraph_renames_without_a_rebuild() {
    let mut t = DOC.as_bytes().to_vec();
    let mut d = MarkdownDoc::new(t.len() as u64);
    complete(&mut d, &t);
    let generation = d.tables_generation();
    let at = DOC.find("More text").unwrap() + 4;
    t.insert(at, b'X');
    d.edit(&t, at as u64, 0, 1).unwrap();
    assert_eq!(d.tables_generation(), generation, "no rebuild");
    assert_eq!(incremental(&mut d), fresh(&t));
}

#[test]
fn heading_text_rebuilds_only_the_outline() {
    let mut t = DOC.as_bytes().to_vec();
    let mut d = MarkdownDoc::new(t.len() as u64);
    complete(&mut d, &t);
    d.tables();
    let at = DOC.find("Part").unwrap();
    t.splice(at..at, b"Title ".iter().copied());
    d.edit(&t, at as u64, 0, 6).unwrap();
    assert!(d.outline_dirty && !d.tables_dirty);
    assert_eq!(incremental(&mut d), fresh(&t));
}

#[test]
fn a_new_footnote_reference_rebuilds() {
    let mut t = DOC.as_bytes().to_vec();
    let mut d = MarkdownDoc::new(t.len() as u64);
    complete(&mut d, &t);
    d.tables();
    let at = DOC.find("See").unwrap();
    t.splice(at..at, b"[^b] ".iter().copied());
    d.edit(&t, at as u64, 0, 5).unwrap();
    assert!(d.tables_dirty);
    assert_eq!(incremental(&mut d), fresh(&t));
}

const PIECES: &[&str] = &[
    "a",
    "x",
    " ",
    "word ",
    "\n",
    "\n\n",
    "# ",
    "## Head\n",
    "===\n",
    "- ",
    "> ",
    "```\n",
    "[x]",
    "[y]",
    "[x]: /u\n",
    "[y]: /v \"t\"\n",
    "[^1]",
    "[^2]",
    "[^1]: n\n",
    "[^2]: m\n\n",
    "Title",
    "| a | b |\n|---|---|\n",
    "한글",
];

fn doc_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(PIECES), 0..50).prop_map(|v| v.concat().into_bytes())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// The tables kept up to date edit by edit equal the tables built from
    /// scratch (positions instead of ids), whichever path each edit took.
    #[test]
    fn incremental_tables_equal_fresh_ones(
        doc in doc_strategy(),
        edits in prop::collection::vec(
            (any::<prop::sample::Index>(), any::<prop::sample::Index>(),
             prop::collection::vec(prop::sample::select(PIECES), 0..3).prop_map(|v| v.concat())),
            1..10,
        ),
        partial in any::<bool>(),
    ) {
        let mut t = doc;
        let mut d = MarkdownDoc::new(t.len() as u64);
        if partial {
            d.extend(&t).unwrap();
        } else {
            complete(&mut d, &t);
        }
        // Tables exist before the edits (the preview asks for them).
        d.tables();
        for (at, len, text) in edits {
            let at = at.index(t.len() + 1);
            let removed = len.index((t.len() - at).min(12) + 1);
            let s = String::from_utf8_lossy(&t).into_owned();
            if s.len() != t.len() || !s.is_char_boundary(at) || !s.is_char_boundary(at + removed) {
                continue;
            }
            t.splice(at..at + removed, text.bytes());
            d.edit(&t, at as u64, removed as u64, text.len() as u64).unwrap();
            if d.index().is_complete() {
                prop_assert_eq!(incremental(&mut d), fresh(&t));
            }
        }
        complete(&mut d, &t);
        prop_assert_eq!(incremental(&mut d), fresh(&t));
    }
}

#[test]
fn typing_in_front_matter_keeps_the_tables() {
    let src = format!("---\ntitle: t\n---\n\n{DOC}");
    let mut t = src.as_bytes().to_vec();
    let mut d = MarkdownDoc::new(t.len() as u64);
    complete(&mut d, &t);
    let generation = d.tables_generation();
    let at = src.find("title").unwrap() + 2;
    t.insert(at, b'X');
    let ch = d.edit(&t, at as u64, 0, 1).unwrap();
    assert!(
        !d.tables_dirty && !d.outline_dirty,
        "{ch:?} {} {}",
        d.tables_dirty,
        d.outline_dirty
    );
    assert_eq!(d.tables_generation(), generation);
    // And front matter is never part of the outline.
    assert_eq!(d.tables().outline()[0].text, "Title");
}
