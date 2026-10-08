use proptest::prelude::*;

use super::*;

/// Buffers held in memory; buffer `unknown` (if any) reports no counts
/// until `known` is set, like an original the index hasn't reached.
struct Bufs {
    bufs: Vec<Vec<u8>>,
    unknown: Option<BufferId>,
}

fn piece_breaks(p: &[u8]) -> Vec<u64> {
    (0..p.len())
        .filter(|&i| p[i] == b'\n' || (p[i] == b'\r' && p.get(i + 1) != Some(&b'\n')))
        .map(|i| i as u64 + 1)
        .collect()
}

impl PieceContext for Bufs {
    fn breaks(&self, buf: BufferId, start: u64, len: u64) -> Option<u64> {
        if self.unknown == Some(buf) {
            return None;
        }
        let b = &self.bufs[buf as usize][start as usize..(start + len) as usize];
        Some(piece_breaks(b).len() as u64)
    }

    fn nth_break(&self, buf: BufferId, start: u64, len: u64, k: u64) -> Option<Option<u64>> {
        if self.unknown == Some(buf) {
            return None;
        }
        let b = &self.bufs[buf as usize][start as usize..(start + len) as usize];
        Some(piece_breaks(b).get(k as usize - 1).map(|&o| start + o))
    }
}

impl Bufs {
    fn content(&self, t: &PieceTree) -> Vec<u8> {
        let mut out = Vec::new();
        for p in t.pieces() {
            out.extend_from_slice(&self.bufs[p.buf as usize][p.start as usize..p.end() as usize]);
        }
        out
    }

    fn piece(&self, buf: BufferId, start: u64, len: u64) -> Piece {
        Piece {
            buf,
            start,
            len,
            breaks: self.breaks(buf, start, len),
        }
    }

    /// Line starts as the tree sees them: breaks counted per piece.
    fn piecewise_line_starts(&self, t: &PieceTree) -> Vec<u64> {
        let mut starts = vec![0];
        let mut base = 0;
        for p in t.pieces() {
            let b = &self.bufs[p.buf as usize][p.start as usize..p.end() as usize];
            starts.extend(piece_breaks(b).into_iter().map(|o| base + o));
            base += p.len;
        }
        starts
    }
}

#[derive(Debug, Clone)]
enum Op {
    Insert { at: f64, bytes: Vec<u8> },
    Delete { at: f64, len: u64 },
    Extend { bytes: Vec<u8> },
}

fn bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![Just(b'\n'), Just(b'\r'), Just(b'a'), Just(b'b')],
        0..12,
    )
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0.0..=1.0f64, bytes()).prop_map(|(at, bytes)| Op::Insert { at, bytes }),
        (0.0..=1.0f64, 0..20u64).prop_map(|(at, len)| Op::Delete { at, len }),
        bytes().prop_map(|bytes| Op::Extend { bytes }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn tree_matches_a_naive_model(
        original in prop::collection::vec(prop_oneof![Just(b'\n'), Just(b'\r'), Just(b'x')], 0..200),
        ops in prop::collection::vec(op(), 0..60),
    ) {
        let mut bufs = Bufs { bufs: vec![original.clone(), Vec::new()], unknown: None };
        let mut tree = PieceTree::from_pieces(&[bufs.piece(0, 0, original.len() as u64)]);
        let mut model = original;
        // Where the last insert ended, for Extend (typing at the same spot).
        let mut cursor = 0u64;
        for op in ops {
            match op {
                Op::Insert { at, bytes } => {
                    let at = (at * model.len() as f64) as u64;
                    let start = bufs.bufs[1].len() as u64;
                    bufs.bufs[1].extend_from_slice(&bytes);
                    let p = bufs.piece(1, start, bytes.len() as u64);
                    tree = tree.insert(at, &PieceTree::from_pieces(&[p]), &bufs);
                    model.splice(at as usize..at as usize, bytes.iter().copied());
                    cursor = at + bytes.len() as u64;
                }
                Op::Delete { at, len } => {
                    let a = (at * model.len() as f64) as u64;
                    let b = (a + len).min(model.len() as u64);
                    let (next, removed) = tree.remove(a, b, &bufs);
                    prop_assert_eq!(bufs.content(&removed), model[a as usize..b as usize].to_vec());
                    tree = next;
                    model.drain(a as usize..b as usize);
                    cursor = cursor.min(model.len() as u64);
                }
                Op::Extend { bytes } => {
                    let start = bufs.bufs[1].len() as u64;
                    bufs.bufs[1].extend_from_slice(&bytes);
                    let p = bufs.piece(1, start, bytes.len() as u64);
                    tree = match tree.extend_at(cursor, &p, &bufs) {
                        Some(t) => t,
                        None => tree.insert(cursor, &PieceTree::from_pieces(&[p]), &bufs),
                    };
                    model.splice(cursor as usize..cursor as usize, bytes.iter().copied());
                    cursor += bytes.len() as u64;
                }
            }
            prop_assert_eq!(bufs.content(&tree), model.clone());
            prop_assert_eq!(tree.len(), model.len() as u64);
        }
        let starts = bufs.piecewise_line_starts(&tree);
        prop_assert_eq!(tree.summary().breaks + 1, starts.len() as u64);
        for (line, &s) in starts.iter().enumerate() {
            prop_assert_eq!(tree.line_start(line as u64, &bufs), Some(Some(s)));
            prop_assert_eq!(tree.breaks_before(s, &bufs), Some(line as u64));
        }
        prop_assert_eq!(tree.line_start(starts.len() as u64, &bufs), Some(None));
        if let Some((start, p)) = tree.piece_at(model.len().saturating_sub(1) as u64) {
            prop_assert!(start + p.len == model.len() as u64);
        }
    }
}

#[test]
fn unknown_regions_answer_none_until_refreshed() {
    let original = b"one\ntwo\nthree\nfour\n".repeat(10);
    let mut bufs = Bufs {
        bufs: vec![original.clone(), b"NEW\n".to_vec()],
        unknown: Some(0),
    };
    let tree = PieceTree::from_pieces(&[bufs.piece(0, 0, original.len() as u64)]);
    let add = PieceTree::from_pieces(&[bufs.piece(1, 0, 4)]);
    let tree = tree.insert(0, &add, &bufs);
    // Line 1 starts right after the inserted line: known.
    assert_eq!(tree.line_start(1, &bufs), Some(Some(4)));
    assert_eq!(tree.breaks_before(4, &bufs), Some(1));
    // Past the unknown original: unknown.
    assert_eq!(tree.line_start(3, &bufs), None);
    assert!(!tree.summary().is_exact());
    bufs.unknown = None;
    let tree = tree.refresh_unknown(&bufs);
    assert!(tree.summary().is_exact());
    assert_eq!(tree.summary().breaks, 41);
    assert_eq!(tree.line_start(3, &bufs), Some(Some(12)));
}

#[test]
fn bulk_built_trees_are_balanced_and_edits_keep_them_shallow() {
    let bufs = Bufs {
        bufs: vec![vec![b'x'; 2_000_000]],
        unknown: None,
    };
    let pieces: Vec<Piece> = (0..1_000_000).map(|i| bufs.piece(0, i * 2, 1)).collect();
    let mut tree = PieceTree::from_pieces(&pieces);
    assert_eq!(tree.piece_count(), 1_000_000);
    assert!(tree.height() <= 21, "height {}", tree.height());
    for i in 0..2000u64 {
        let at = (i * 7919) % tree.len();
        let (t, _) = tree.remove(at, at + 3, &bufs);
        tree = t.insert(
            at / 2,
            &PieceTree::from_pieces(&[bufs.piece(0, 1, 1)]),
            &bufs,
        );
    }
    assert!(tree.height() <= 60, "height {}", tree.height());
}
