use super::*;
use crate::document::{Action, TextDocument};
use crate::search::Query;

fn run(doc: &[u8], pattern: &str, regex: bool, replacement: &str) -> (TextDocument, u64, bool) {
    let mut d = TextDocument::from_bytes(doc);
    let m = Matcher::new(&Query {
        pattern: pattern.into(),
        regex,
        case_sensitive: true,
        whole_word: false,
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let r = compute(
        &d.snapshot(),
        &d.buffer().adder(),
        &m,
        replacement,
        dir.path(),
        &CancelToken::new(),
        &|_| {},
    )
    .unwrap()
    .unwrap();
    let (count, mat) = (r.count, r.materialized);
    d.swap_tree(
        r.tree,
        crate::document::Record::ReplaceAll {
            query: m.query.clone(),
            replacement: replacement.into(),
        },
    );
    (d, count, mat)
}

fn content(d: &TextDocument) -> Vec<u8> {
    d.snapshot().read(0..d.len()).unwrap()
}

fn expected(doc: &[u8], pattern: &str, replacement: &str) -> Vec<u8> {
    let re = regex_automata::meta::Regex::new(pattern).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    for m in re.find_iter(doc).filter(|m| !m.is_empty()) {
        out.extend_from_slice(&doc[at..m.start()]);
        out.extend_from_slice(replacement.as_bytes());
        at = m.end();
    }
    out.extend_from_slice(&doc[at..]);
    out
}

#[test]
fn pieces_and_materialized_results_match_and_undo_in_one_step() {
    let doc: Vec<u8> = (0..3000)
        .map(|i| format!("line {i} has a cat\r\n"))
        .collect::<String>()
        .into_bytes();
    for (pattern, repl, materialized) in [
        ("line 2[0-4] ", "L ", false),
        ("cat", "dog", true),
        ("a", "", true),
    ] {
        let (mut d, count, mat) = run(&doc, pattern, true, repl);
        assert_eq!(mat, materialized, "{pattern}");
        assert!(count > 0);
        let want = expected(&doc, pattern, repl);
        assert_eq!(content(&d), want);
        // Line structure stays exact (CRLF counted once).
        let lines = want.windows(2).filter(|w| w == b"\r\n").count() as u64 + 1;
        assert_eq!(d.snapshot().line_count(), Some(lines));
        d.apply(&Action::Undo).unwrap();
        assert_eq!(content(&d), doc);
        d.apply(&Action::Redo).unwrap();
        assert_eq!(content(&d), want);
    }
}

#[test]
fn replacements_that_join_cr_and_lf_count_one_break() {
    let (d, _, _) = run(b"a\rX\nb", "X", false, "");
    assert_eq!(content(&d), b"a\r\nb");
    assert_eq!(d.snapshot().line_count(), Some(2));
}

#[test]
fn replay_from_the_journal_record() {
    let doc = b"one two one two one".to_vec();
    let (mut d, _, _) = run(&doc, "one", false, "1");
    let records = d.take_records();
    let mut replay = TextDocument::from_bytes(&doc);
    for r in &records {
        replay.apply_record(r).unwrap();
    }
    assert_eq!(content(&replay), b"1 two 1 two 1");
}
