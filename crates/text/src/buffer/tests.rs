use proptest::prelude::*;

use super::*;
use crate::source::MemorySource;

/// Line starts of a document: after each LF, and after each CR not
/// followed by an LF.
fn line_starts(doc: &[u8]) -> Vec<u64> {
    let mut v = vec![0];
    for i in 0..doc.len() {
        if doc[i] == b'\n' || (doc[i] == b'\r' && doc.get(i + 1) != Some(&b'\n')) {
            v.push(i as u64 + 1);
        }
    }
    v
}

fn content(b: &TextBuffer) -> Vec<u8> {
    b.snapshot().read(0..b.len()).unwrap()
}

#[derive(Debug, Clone)]
struct Edit {
    at: f64,
    del: u64,
    text: Vec<u8>,
}

fn edit() -> impl Strategy<Value = Edit> {
    (
        0.0..=1.0f64,
        0..8u64,
        prop::collection::vec(prop_oneof![Just(b'\n'), Just(b'\r'), Just(b'z')], 0..6),
    )
        .prop_map(|(at, del, text)| Edit { at, del, text })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// The buffer matches a plain byte vector, its line structure matches
    /// the document's (CRLF counted once, even when an edit joins a CR and
    /// an LF from different pieces), and putting removed pieces back
    /// restores the previous content exactly.
    #[test]
    fn buffer_matches_model_and_removals_invert(
        original in prop::collection::vec(prop_oneof![Just(b'\n'), Just(b'\r'), Just(b'o')], 0..120),
        edits in prop::collection::vec(edit(), 0..40),
    ) {
        let mut buf = TextBuffer::from_bytes(&original);
        let mut model = original;
        for e in edits {
            let at = (e.at * model.len() as f64) as u64;
            let to = (at + e.del).min(model.len() as u64);
            let before = model.clone();
            let removed = buf.replace(at, to, &e.text).unwrap();
            model.splice(at as usize..to as usize, e.text.iter().copied());
            prop_assert_eq!(content(&buf), model.clone());

            let starts = line_starts(&model);
            let snap = buf.snapshot();
            prop_assert_eq!(snap.line_count(), Some(starts.len() as u64));
            for (line, &s) in starts.iter().enumerate() {
                prop_assert_eq!(snap.line_start(line as u64), Some(Some(s)));
                prop_assert_eq!(snap.line_of(s), Some(line as u64));
                prop_assert_eq!(snap.line_start_containing(s).unwrap(), s);
            }

            // Undo then redo through the pieces.
            if e.at < 0.3 {
                buf.remove(at, at + e.text.len() as u64).unwrap();
                buf.insert_pieces(at, &removed.pieces).unwrap();
                prop_assert_eq!(content(&buf), before.clone());
                prop_assert_eq!(buf.snapshot().line_count(), Some(line_starts(&before).len() as u64));
                buf.replace(at, to, &e.text).unwrap();
                prop_assert_eq!(content(&buf), model.clone());
            }
        }
    }
}

#[test]
fn line_ends_and_containing_starts() {
    let buf = TextBuffer::from_bytes(b"ab\r\ncd\ref\n\ngh");
    let s = buf.snapshot();
    assert_eq!(s.line_end(0).unwrap(), (2, 4));
    assert_eq!(s.line_end(4).unwrap(), (6, 7));
    assert_eq!(s.line_end(7).unwrap(), (9, 10));
    assert_eq!(s.line_end(10).unwrap(), (10, 11));
    assert_eq!(s.line_end(11).unwrap(), (13, 13));
    assert_eq!(s.line_start_containing(5).unwrap(), 4);
    assert_eq!(s.line_start_containing(12).unwrap(), 11);
    assert_eq!(s.line_count(), Some(5));
}

#[test]
fn estimates_before_indexing_land_on_line_starts() {
    let lines: Vec<String> = (0..20_000)
        .map(|i| format!("{i:>8}| some text\n"))
        .collect();
    let doc = lines.concat().into_bytes();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(doc.clone()));
    let index = Arc::new(ChunkIndex::unindexed(src.len()));
    let mut buf = TextBuffer::new(Arc::clone(&src), Arc::clone(&index));
    let s = buf.snapshot();
    assert_eq!(s.line_count(), None);
    let est = s.line_count_estimate();
    assert!(!est.exact);
    let starts = line_starts(&doc);
    for target in [0u64, 1, 5000, 12_345, 19_999, 50_000] {
        let (off, exact) = s.line_start_estimate(target);
        assert!(!exact || target == 0);
        assert!(
            starts.binary_search(&off).is_ok(),
            "{off} is not a line start"
        );
        // Uniform lines: the estimate is close.
        let got = starts.binary_search(&off).unwrap() as i64;
        assert!(
            (got - target.min(19_999) as i64).abs() <= 100,
            "{target} -> {got}"
        );
    }
    let l = s.line_of_estimate(doc.len() as u64 / 2);
    assert!(!l.exact);
    assert!((l.line as i64 - 10_000).abs() <= 100);

    index
        .scan_range(&*src, 0, index.chunk_count(), None)
        .unwrap();
    buf.refresh_index();
    let s = buf.snapshot();
    assert_eq!(s.line_count(), Some(20_001));
    assert_eq!(s.line_start_estimate(12_345), (starts[12_345], true));
    assert_eq!(s.line_start_estimate(99_999), (starts[20_000], true));
}

#[test]
fn add_buffer_reads_across_blocks() {
    let add = AddBuffer::default();
    let a = vec![7u8; BLOCK - 3];
    add.append(&a);
    let at = add.append(b"0123456789");
    assert_eq!(at, (BLOCK - 3) as u64);
    let mut buf = [0u8; 10];
    add.read_at(at, &mut buf).unwrap();
    assert_eq!(&buf, b"0123456789");
    assert!(add.read_at(at + 5, &mut buf).is_err());
}

#[test]
fn long_line_scans_match_the_sequential_scan() {
    // A 40 MiB line (two 32 MiB stages, each split in four parts) with a
    // break placed at seams: lone CR, LF and CRLF straddling a boundary.
    const MIB: usize = 1 << 20;
    let len = 40 * MIB;
    let part = (32 * MIB).div_ceil(4);
    let cases: &[(usize, &[u8])] = &[
        (5, b"\n"),
        (part - 1, b"\r\n"),
        (part, b"\r"),
        (2 * part + 3, b"\n"),
        (32 * MIB - 1, b"\r\n"),
        (32 * MIB, b"\n"),
        (len - 1, b"\r"),
    ];
    for &(at, brk) in cases {
        let mut bytes = vec![b'a'; len];
        bytes[at..at + brk.len()].copy_from_slice(brk);
        let s = TextBuffer::from_bytes(&bytes).snapshot();
        let n = s.len();
        for from in [
            0,
            at.saturating_sub(1) as u64,
            (at + brk.len()) as u64,
            n / 2,
        ] {
            assert_eq!(
                s.scan_line_end(from, None).unwrap(),
                s.scan_line_end(from, Some(n)).unwrap(),
                "end from {from}, break {brk:?} at {at}"
            );
        }
        for to in [
            n,
            (at + brk.len()) as u64,
            at as u64 + 1,
            (at + 1).min(len - 1) as u64,
        ] {
            assert_eq!(
                s.scan_back_for_line_start(to, None).unwrap(),
                s.scan_back_for_line_start(to, Some(to)).unwrap(),
                "start before {to}, break {brk:?} at {at}"
            );
        }
    }
}

#[test]
fn line_end_scans_skip_what_the_index_knows() {
    // A 4 MiB line with breaks near chunk seams; the index knows some
    // chunks (a prefix, and every third one), the scan must still find
    // exactly what a plain scan finds, both ways.
    const C: usize = crate::index::CHUNK as usize;
    let len = 64 * C;
    let cases: &[(usize, &[u8])] = &[
        (10 * C - 1, b"\r\n"),
        (20 * C - 1, b"\r"),
        (30 * C, b"\n"),
        (len - 1, b"\n"),
        (3, b"\r\n"),
    ];
    for &(at, brk) in cases {
        let mut bytes = vec![b'a'; len];
        bytes[at..at + brk.len()].copy_from_slice(brk);
        let src: Arc<dyn Source> = Arc::new(MemorySource::new(bytes.clone()));
        let index = Arc::new(ChunkIndex::unindexed(src.len()));
        let n = index.chunk_count();
        index.scan_range(&*src, 0, n / 2, None).unwrap();
        for k in (n / 2..n).step_by(3) {
            index.scan_range(&*src, k, k + 1, None).unwrap();
        }
        let s = TextBuffer::new(Arc::clone(&src), Arc::clone(&index)).snapshot();
        let plain = |from: usize, back: bool| -> Option<usize> {
            if back {
                bytes[..from]
                    .iter()
                    .rposition(|&b| b == b'\n' || b == b'\r')
            } else {
                bytes[from..]
                    .iter()
                    .position(|&b| b == b'\n' || b == b'\r')
                    .map(|p| p + from)
            }
        };
        for from in [0, C / 2, 9 * C, 10 * C - 1, 10 * C + 1, 40 * C, len - 2] {
            let got = s.find_cr_lf(from as u64..len as u64, false).unwrap();
            assert_eq!(
                got,
                plain(from, false).map(|p| p as u64),
                "forward from {from}, {brk:?} at {at}"
            );
        }
        for to in [len, 33 * C, 20 * C, 10 * C + 1, 10 * C, 5] {
            let got = s.find_cr_lf(0..to as u64, true).unwrap();
            assert_eq!(
                got,
                plain(to, true).map(|p| p as u64),
                "backward to {to}, {brk:?} at {at}"
            );
        }
    }
}
