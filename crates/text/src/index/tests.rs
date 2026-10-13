use super::*;
use crate::source::MemorySource;

/// Breaks of `p` taken as a whole piece: LF, or CR not followed by LF
/// inside `p` (a CR at the end counts).
fn naive_breaks(p: &[u8]) -> Vec<usize> {
    (0..p.len())
        .filter(|&i| p[i] == b'\n' || (p[i] == b'\r' && p.get(i + 1) != Some(&b'\n')))
        .map(|i| i + 1)
        .collect()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn text(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| match rng.below(20) {
            0 => b'\n',
            1 => b'\r',
            2 => 0,
            _ => b'a' + rng.below(26) as u8,
        })
        .collect()
}

#[test]
fn scan_counts_crlf_once_and_lone_cr() {
    let (n, s) = scan(b"a\r\nb\rc\nd\r", None, None);
    assert_eq!(n, 4);
    assert_eq!((s.lf, s.crlf, s.cr), (1, 1, 2));
    // A CRLF split across two scans: the CR isn't a break, and the scan
    // holding the LF counts the CRLF.
    let (n, s) = scan(b"a\r", None, Some(b'\n'));
    assert_eq!((n, s.lf, s.crlf, s.cr), (0, 0, 0, 0));
    let (n, s) = scan(b"\nb", Some(b'\r'), None);
    assert_eq!((n, s.lf, s.crlf, s.cr), (1, 0, 1, 0));
}

#[test]
fn piece_queries_match_the_naive_model() {
    let mut rng = Rng(0x1234_5678);
    let data = text(&mut rng, 5 * CHUNK as usize + 777);
    let src = MemorySource::new(data.clone());
    let index = ChunkIndex::build(&src).unwrap();
    assert!(index.is_complete());
    assert_eq!(index.known_chunks(), index.chunk_count());
    let len = data.len() as u64;
    let mut ranges = vec![(0, len), (0, 0), (CHUNK, 2 * CHUNK), (CHUNK - 1, CHUNK + 1)];
    for _ in 0..120 {
        let a = rng.below(len + 1);
        let b = a + rng.below(len - a + 1);
        ranges.push((a, b));
    }
    for (a, b) in ranges {
        let piece = &data[a as usize..b as usize];
        let expected = naive_breaks(piece);
        let got = index.piece_breaks(&src, a, b).unwrap().unwrap();
        assert_eq!(got, expected.len() as u64, "piece {a}..{b}");
        let step = (expected.len() / 4).max(1);
        for (i, &e) in expected.iter().enumerate().step_by(step) {
            let hit = index
                .piece_nth_break(&src, a, b, i as u64 + 1)
                .unwrap()
                .unwrap();
            assert_eq!(hit, Some(a + e as u64), "nth {} of {a}..{b}", i + 1);
        }
        let past = index
            .piece_nth_break(&src, a, b, expected.len() as u64 + 1)
            .unwrap()
            .unwrap();
        assert_eq!(past, None);
    }
}

#[test]
fn unknown_chunks_give_none_and_partial_spans_still_scan() {
    let mut rng = Rng(99);
    let data = text(&mut rng, 4 * CHUNK as usize);
    let src = MemorySource::new(data.clone());
    let index = ChunkIndex::unindexed(src.len());
    // Inside one chunk, no index is needed.
    let small = index.piece_breaks(&src, 10, 5000).unwrap();
    assert_eq!(small, Some(naive_breaks(&data[10..5000]).len() as u64));
    assert_eq!(index.piece_breaks(&src, 0, 3 * CHUNK).unwrap(), None);
    index.scan_range(&src, 0, 2, None).unwrap();
    assert!(
        index
            .piece_breaks(&src, 5, 2 * CHUNK - 3)
            .unwrap()
            .is_some()
    );
    assert_eq!(index.piece_breaks(&src, 5, 3 * CHUNK + 1).unwrap(), None);
    let p = index.progress();
    assert!(!p.complete);
    assert_eq!(p.known_bytes, 2 * CHUNK);
}

#[test]
fn growing_buffers_extend_their_index() {
    let mut rng = Rng(7);
    let mut data = Vec::new();
    let mut index = ChunkIndex::unindexed(0);
    for step in 0..40 {
        let n = rng.below(20_000) as usize;
        let mut add = text(&mut rng, n);
        if step % 5 == 0 {
            // Exercise a CR at the end followed later by an LF.
            add.push(b'\r');
        }
        data.extend_from_slice(&add);
        let src = MemorySource::new(data.clone());
        if step % 13 == 0 {
            index = ChunkIndex::build(&src).unwrap();
        } else {
            index.extend(&src, data.len() as u64).unwrap();
        }
        let got = index.piece_breaks(&src, 0, data.len() as u64).unwrap();
        assert_eq!(got, Some(naive_breaks(&data).len() as u64));
        let p = index.progress();
        assert!(p.complete);
        assert_eq!(p.endings.breaks(), naive_breaks(&data).len() as u64);
    }
}

#[test]
fn background_indexing_completes_and_reports_progress() {
    use quire_core::scheduler::{CancelToken, Scheduler, SchedulerConfig};
    let mut rng = Rng(5);
    let data: Arc<[u8]> = text(&mut rng, 3_000_000).into();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(Arc::clone(&data)));
    let index = Arc::new(ChunkIndex::unindexed(src.len()));
    let scheduler = Scheduler::new(SchedulerConfig::default());
    let calls = Arc::new(AtomicU64::new(0));
    let c2 = Arc::clone(&calls);
    let handles = index_in_background(
        &scheduler,
        Arc::clone(&src),
        Arc::clone(&index),
        CancelToken::new(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        Arc::new(move |_| {
            c2.fetch_add(1, Ordering::Relaxed);
        }),
    );
    for h in handles {
        h.wait().unwrap();
    }
    assert!(index.is_complete());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        index.piece_breaks(&*src, 0, src.len()).unwrap(),
        Some(naive_breaks(&data).len() as u64)
    );
    assert!(index.fingerprint(0).is_some());
}
