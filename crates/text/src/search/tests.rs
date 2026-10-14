use proptest::prelude::*;
use quire_core::scheduler::SchedulerConfig;

use super::*;
use crate::buffer::TextBuffer;

fn q(pattern: &str, regex: bool) -> Query {
    Query {
        pattern: pattern.into(),
        regex,
        case_sensitive: true,
        whole_word: false,
    }
}

/// The reference: one sequential search over the whole document.
fn naive(doc: &[u8], m: &Matcher) -> Vec<(u64, u64)> {
    m.regex()
        .find_iter(doc)
        .filter(|x| !x.is_empty())
        .map(|x| (x.start() as u64, x.end() as u64))
        .collect()
}

fn snap(doc: &[u8]) -> Snapshot {
    TextBuffer::from_bytes(doc).snapshot()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 60, ..ProptestConfig::default() })]

    /// Chunked, parallel search equals a sequential search of the whole
    /// buffer, for patterns that cross chunk seams, overlap one another, are
    /// longer than the overlap window, or look behind/ahead (\b, ^, $).
    #[test]
    fn chunked_search_equals_whole_buffer_search(
        doc in prop::collection::vec(prop_oneof![Just(b'a'), Just(b'b'), Just(b' '), Just(b'\n')], 0..20_000),
        p in 0..7usize,
    ) {
        let patterns = [("ab", false), ("aa", true), ("a[ab]*b", true), (r"\bab\b", true), ("^b", true), ("a$", true), ("b{100,}", true)];
        let (pat, regex) = patterns[p];
        let m = Matcher::new(&q(pat, regex)).unwrap();
        let s = snap(&doc);
        let expected = naive(&doc, &m);
        prop_assert_eq!(search_range(&s, &m, 0, s.len(), usize::MAX).unwrap(), expected.clone());
        let scheduler = Scheduler::new(SchedulerConfig { threads: 3, urgent_reserved: 1, history: 0, thread_name: "t" });
        let mut got = Vec::new();
        let p = search_all(&s, &m, &scheduler, 2, &CancelToken::new(), &mut |b, _| got.extend_from_slice(b)).unwrap();
        prop_assert!(p.complete);
        prop_assert_eq!(p.matches, expected.len() as u64);
        prop_assert_eq!(got, expected.clone());
        if let Some(&first) = expected.first() {
            prop_assert_eq!(find_next(&s, &m, 0).unwrap(), Some(first));
            prop_assert_eq!(find_prev(&s, &m, s.len()).unwrap(), expected.last().copied());
        }
    }
}

#[test]
fn case_whole_word_and_unicode() {
    let s = snap("Straße STRASSE strasse Cat cat catalog 고양이 고양이들".as_bytes());
    let mut query = q("cat", false);
    query.case_sensitive = false;
    let m = Matcher::new(&query).unwrap();
    assert_eq!(search_range(&s, &m, 0, s.len(), 10).unwrap().len(), 3);
    query.whole_word = true;
    let m = Matcher::new(&query).unwrap();
    assert_eq!(search_range(&s, &m, 0, s.len(), 10).unwrap().len(), 2);
    let m = Matcher::new(&Query {
        whole_word: true,
        ..q("고양이", false)
    })
    .unwrap();
    assert_eq!(search_range(&s, &m, 0, s.len(), 10).unwrap().len(), 1);
    assert!(Matcher::new(&q("(", true)).is_err());
    assert!(Matcher::new(&q("", false)).is_err());
}

#[test]
fn invalid_utf8_is_searchable() {
    let s = snap(b"ok \xFF\xFE bad ok");
    let m = Matcher::new(&q("ok", false)).unwrap();
    assert_eq!(
        search_range(&s, &m, 0, s.len(), 10).unwrap(),
        vec![(0, 2), (10, 12)]
    );
}

#[test]
fn expansion_uses_captures() {
    let s = snap(b"key=value; k2=v2");
    let m = Matcher::new(&q(r"(\w+)=(?P<v>\w+)", true)).unwrap();
    let hits = search_range(&s, &m, 0, s.len(), 10).unwrap();
    assert_eq!(expand(&s, &m, hits[0], "${v}:$1").unwrap(), b"value:key");
    let lit = Matcher::new(&q("k2", false)).unwrap();
    assert_eq!(expand(&s, &lit, (11, 13), "$1").unwrap(), b"$1");
}

#[test]
fn find_prev_returns_the_forward_searchs_match_when_matches_overlap() {
    // Fuzz regression (search): a run of quotes and a 3-quote pattern; a
    // scan starting mid-run sees matches the forward search skips.
    let mut doc = vec![b'"'; 42];
    doc.extend_from_slice(b"KKK?:KKK\x1aRK\0\0/\0KKKKK\xcab\n\n");
    let m = Matcher::new(&q("\"\"\"", true)).unwrap();
    assert!(!m.spans_lines);
    let s = snap(&doc);
    let all = naive(&doc, &m);
    assert_eq!(
        find_prev(&s, &m, doc.len() as u64).unwrap(),
        all.last().copied()
    );
    // Long runs over many chunks, and a line before them.
    let mut long = b"x\n".to_vec();
    long.extend(std::iter::repeat_n(b'a', 3 * CHUNK as usize + 7));
    let m = Matcher::new(&q("aa", false)).unwrap();
    let s = snap(&long);
    let all = naive(&long, &m);
    for before in [long.len() as u64, long.len() as u64 - 1, CHUNK + 5, 3] {
        // Nothing before: it wraps to the last match.
        let want = all
            .iter()
            .rev()
            .find(|h| h.1 <= before)
            .or(all.last())
            .copied();
        assert_eq!(find_prev(&s, &m, before).unwrap(), want, "before {before}");
    }
    // A pattern that can match a line break scans from the document's start.
    let m = Matcher::new(&q(r"a\s?a", true)).unwrap();
    assert!(m.spans_lines);
    let all = naive(&long, &m);
    assert_eq!(
        find_prev(&s, &m, long.len() as u64).unwrap(),
        all.last().copied()
    );
}

#[test]
fn a_match_cut_at_a_chunk_end_is_never_invented() {
    // Fuzz regression (search): `.*` up to the end of a chunk's haystack;
    // with the invalid UTF-8 byte after it the engine reports no match at
    // the start, and the search must not report one either.
    let mut doc = vec![b'a'; (CHUNK + OVERLAP) as usize];
    doc.push(0x98);
    let m = Matcher::new(&q(".*", true)).unwrap();
    let s = snap(&doc);
    assert_eq!(
        search_range(&s, &m, 0, doc.len() as u64, usize::MAX).unwrap(),
        naive(&doc, &m)
    );
}
