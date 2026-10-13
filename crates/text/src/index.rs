//! Per-buffer line index (§6.1): line-break counts per 64 KiB chunk, not
//! per-line offsets, so the index of a 2 GB file stays a few hundred KiB.
//!
//! A **break position** is a byte that ends a line: an LF, or a CR that isn't
//! followed by an LF. CRLF therefore counts once (at its LF), and a lone CR
//! (classic Mac) counts as a break. The piece tree counts breaks per piece;
//! see [`ChunkIndex::piece_breaks`] for how a piece that ends between a CR
//! and an LF is counted.
//!
//! The index is progressive: chunks become known as the background scan
//! (`scan_range`) gets to them, and everything that needs an unknown chunk
//! answers `None`, so callers fall back to estimates.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use serde::Serialize;
use xxhash_rust::xxh3::xxh3_64;

use crate::source::Source;

/// Index granularity (§6.1: 64 KB chunks).
pub const CHUNK: u64 = 64 * 1024;
const UNKNOWN: u32 = u32::MAX;

/// Line-ending statistics, for the status bar (§6.1: flag mixed endings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndingStats {
    pub lf: u64,
    pub crlf: u64,
    /// CRs not followed by an LF.
    pub cr: u64,
    /// NUL bytes, for binary detection.
    pub nul: u64,
}

impl EndingStats {
    fn add(&mut self, o: &EndingStats) {
        self.lf += o.lf;
        self.crlf += o.crlf;
        self.cr += o.cr;
        self.nul += o.nul;
    }

    fn sub(&mut self, o: &EndingStats) {
        self.lf -= o.lf;
        self.crlf -= o.crlf;
        self.cr -= o.cr;
        self.nul -= o.nul;
    }

    /// Breaks counted by these stats.
    pub fn breaks(&self) -> u64 {
        self.lf + self.crlf + self.cr
    }
}

/// Counts break positions in `bytes`. `prev` and `next` are the bytes
/// around `bytes` in the buffer (`None` at either end): `next` decides
/// whether a final CR is a break, `prev` whether a leading LF ends a CRLF
/// (which only matters for the statistics). Also returns the line-ending
/// statistics; a CRLF split across two scans is counted by the scan holding
/// its LF.
pub fn scan(bytes: &[u8], prev: Option<u8>, next: Option<u8>) -> (u64, EndingStats) {
    let lf_total = memchr::memchr_iter(b'\n', bytes).count() as u64;
    let mut crlf_inside = 0u64;
    let mut cr = 0u64;
    for p in memchr::memchr_iter(b'\r', bytes) {
        match bytes.get(p + 1) {
            Some(b'\n') => crlf_inside += 1,
            Some(_) => cr += 1,
            None if next == Some(b'\n') => {}
            None => cr += 1,
        }
    }
    let crlf_in = u64::from(bytes.first() == Some(&b'\n') && prev == Some(b'\r'));
    let s = EndingStats {
        lf: lf_total - crlf_inside - crlf_in,
        crlf: crlf_inside + crlf_in,
        cr,
        nul: memchr::memchr_iter(0, bytes).count() as u64,
    };
    (lf_total + cr, s)
}

/// Offset just past the `k`-th (1-based) break position in `bytes`.
pub fn nth_break(bytes: &[u8], next: Option<u8>, k: u64) -> Option<usize> {
    debug_assert!(k >= 1);
    let mut seen = 0;
    for p in memchr::memchr2_iter(b'\n', b'\r', bytes) {
        let is_break = bytes[p] == b'\n' || bytes.get(p + 1).copied().or(next) != Some(b'\n');
        if is_break {
            seen += 1;
            if seen == k {
                return Some(p + 1);
            }
        }
    }
    None
}

struct State {
    len: u64,
    counts: Vec<u32>,
    stats: Vec<EndingStats>,
    fingerprints: Vec<u64>,
    known: usize,
    /// Prefix sums of `counts` (len + 1 entries) once every chunk is known.
    prefix: Option<Vec<u64>>,
    totals: EndingStats,
}

impl State {
    fn chunks(len: u64) -> usize {
        len.div_ceil(CHUNK) as usize
    }

    fn set(&mut self, i: usize, count: u64, stats: EndingStats, fp: u64) {
        if self.counts[i] == UNKNOWN {
            self.known += 1;
        } else {
            let old = self.stats[i];
            self.totals.sub(&old);
        }
        self.counts[i] = count as u32;
        self.stats[i] = stats;
        self.fingerprints[i] = fp;
        self.totals.add(&stats);
        if self.known == self.counts.len() {
            let mut prefix = Vec::with_capacity(self.counts.len() + 1);
            let mut sum = 0u64;
            prefix.push(0);
            for &c in &self.counts {
                sum += u64::from(c);
                prefix.push(sum);
            }
            self.prefix = Some(prefix);
        } else {
            self.prefix = None;
        }
    }

    /// Walks chunks `c..last` while their counts stay below `remaining`:
    /// returns the first chunk whose count reaches it (or `last`) and the
    /// breaks in the chunks before it. `None` if an unknown chunk is in the
    /// way. A binary search over the prefix sums once complete.
    fn skip(&self, c: usize, last: usize, remaining: u64) -> Option<(usize, u64)> {
        if let Some(p) = &self.prefix {
            let target = p[c] + remaining;
            let j = c + p[c + 1..=last].partition_point(|&s| s < target);
            return Some((j, p[j] - p[c]));
        }
        let mut sum = 0u64;
        for j in c..last {
            let n = self.counts[j];
            if n == UNKNOWN {
                return None;
            }
            if sum + u64::from(n) >= remaining {
                return Some((j, sum));
            }
            sum += u64::from(n);
        }
        Some((last, sum))
    }

    /// Sum of counts of chunks `lo..hi`, if all are known.
    fn sum(&self, lo: usize, hi: usize) -> Option<u64> {
        if lo >= hi {
            return Some(0);
        }
        if let Some(p) = &self.prefix {
            return Some(p[hi] - p[lo]);
        }
        let mut s = 0u64;
        for &c in &self.counts[lo..hi] {
            if c == UNKNOWN {
                return None;
            }
            s += u64::from(c);
        }
        Some(s)
    }
}

/// Progress of an index, for the status bar.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub bytes: u64,
    pub known_bytes: u64,
    pub complete: bool,
    /// Breaks counted so far.
    pub known_breaks: u64,
    pub endings: EndingStats,
}

/// The line index of one buffer. See the module docs.
pub struct ChunkIndex {
    state: RwLock<State>,
    /// Bumped whenever a chunk becomes known.
    version: AtomicU64,
}

impl std::fmt::Debug for ChunkIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.state.read();
        f.debug_struct("ChunkIndex")
            .field("len", &st.len)
            .field("known", &st.known)
            .field("chunks", &st.counts.len())
            .finish()
    }
}

/// Builds an index from a byte stream as it is written (the save pipeline
/// indexes the new file while writing it, so line numbers stay exact after
/// a save without re-reading the file).
#[derive(Debug, Default)]
pub struct IndexBuilder {
    pending: Vec<u8>,
    prev: Option<u8>,
    chunks: Vec<(u64, EndingStats, u64)>,
    len: u64,
}

impl IndexBuilder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.len += bytes.len() as u64;
        self.pending.extend_from_slice(bytes);
        let c = CHUNK as usize;
        // A chunk is final once the byte after it is known.
        let mut at = 0;
        while self.pending.len() - at > c {
            let chunk = &self.pending[at..at + c];
            let (n, s) = scan(chunk, self.prev, Some(self.pending[at + c]));
            self.chunks.push((n, s, xxh3_64(chunk)));
            self.prev = chunk.last().copied();
            at += c;
        }
        self.pending.drain(..at);
    }

    pub fn finish(mut self) -> ChunkIndex {
        if !self.pending.is_empty() {
            let (n, s) = scan(&self.pending, self.prev, None);
            self.chunks.push((n, s, xxh3_64(&self.pending)));
        }
        let len = self.len;
        let index = ChunkIndex::unindexed(len);
        {
            let mut st = index.state.write();
            for (i, (n, s, fp)) in self.chunks.into_iter().enumerate() {
                st.set(i, n, s, fp);
            }
        }
        index
    }
}

impl ChunkIndex {
    /// An index of `len` bytes with nothing known yet.
    pub fn unindexed(len: u64) -> Self {
        let n = State::chunks(len);
        Self {
            state: RwLock::new(State {
                len,
                counts: vec![UNKNOWN; n],
                stats: vec![EndingStats::default(); n],
                fingerprints: vec![0; n],
                known: 0,
                prefix: (n == 0).then(|| vec![0]),
                totals: EndingStats::default(),
            }),
            version: AtomicU64::new(0),
        }
    }

    /// Indexes all of `source` synchronously (small buffers, tests).
    pub fn build(source: &dyn Source) -> io::Result<Self> {
        let index = Self::unindexed(source.len());
        index.scan_range(source, 0, index.chunk_count(), None)?;
        Ok(index)
    }

    pub fn len(&self) -> u64 {
        self.state.read().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn chunk_count(&self) -> usize {
        self.state.read().counts.len()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        let st = self.state.read();
        st.known == st.counts.len()
    }

    pub fn progress(&self) -> IndexProgress {
        let st = self.state.read();
        let known_bytes = if st.known == st.counts.len() {
            st.len
        } else {
            (st.known as u64 * CHUNK).min(st.len)
        };
        IndexProgress {
            bytes: st.len,
            known_bytes,
            complete: st.known == st.counts.len(),
            known_breaks: st.totals.breaks(),
            endings: st.totals,
        }
    }

    /// Fingerprint of chunk `i` (xxh3 of its bytes), once indexed.
    pub fn fingerprint(&self, i: usize) -> Option<u64> {
        let st = self.state.read();
        (st.counts.get(i).copied()? != UNKNOWN).then(|| st.fingerprints[i])
    }

    /// Breaks per byte over the indexed part, for estimates.
    pub fn density(&self) -> Option<f64> {
        let p = self.progress();
        (p.known_bytes > 0).then(|| p.known_breaks as f64 / p.known_bytes as f64)
    }

    /// Scans chunks `lo..hi` of `source` and records them. `cancel` is
    /// polled between 1 MiB blocks; returns early (Ok) if it says stop.
    pub fn scan_range(
        &self,
        source: &dyn Source,
        lo: usize,
        hi: usize,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> io::Result<()> {
        self.scan_range_ahead(source, lo, hi, cancel, 0)
    }

    /// [`scan_range`](Self::scan_range), prefetching up to `ahead` bytes
    /// past the block being read (0: no prefetch). A short window keeps the
    /// device queue short, so foreground reads don't wait behind it.
    pub fn scan_range_ahead(
        &self,
        source: &dyn Source,
        lo: usize,
        hi: usize,
        cancel: Option<&dyn Fn() -> bool>,
        ahead: u64,
    ) -> io::Result<()> {
        const BLOCK_CHUNKS: usize = 16; // 1 MiB reads
        let len = source.len();
        let range_end = (hi as u64 * CHUNK).min(len);
        let mut prefetched = lo as u64 * CHUNK;
        let mut buf = vec![0u8; BLOCK_CHUNKS * CHUNK as usize + 1];
        let mut i = lo;
        while i < hi {
            if cancel.is_some_and(|c| c()) {
                return Ok(());
            }
            let n = BLOCK_CHUNKS.min(hi - i);
            let start = i as u64 * CHUNK;
            let end = ((i + n) as u64 * CHUNK).min(len);
            // Top the window up once it's half used.
            if ahead > 0 && prefetched < range_end && prefetched < end + ahead / 2 {
                let to = (end + ahead).min(range_end);
                let from = prefetched.max(start);
                source.prefetch(from, to - from);
                prefetched = to;
            }
            // One byte of look-ahead past the block, for a trailing CR.
            let read_end = (end + 1).min(len);
            let block = &mut buf[..(read_end - start) as usize];
            source.read_at(start, block)?;
            let prev_byte = if start > 0 {
                let mut one = [0u8];
                source.read_at(start - 1, &mut one)?;
                Some(one[0])
            } else {
                None
            };
            let mut results = Vec::with_capacity(n);
            for c in 0..n {
                let a = c * CHUNK as usize;
                let b = ((c + 1) * CHUNK as usize).min((end - start) as usize);
                let bytes = &block[a..b];
                let next = block.get(b).copied();
                let prev = if a > 0 { Some(block[a - 1]) } else { prev_byte };
                let (count, stats) = scan(bytes, prev, next);
                results.push((i + c, count, stats, xxh3_64(bytes)));
            }
            {
                let mut st = self.state.write();
                for (k, count, stats, fp) in results {
                    st.set(k, count, stats, fp);
                }
            }
            self.version.fetch_add(1, Ordering::AcqRel);
            i += n;
        }
        Ok(())
    }

    /// For growing buffers: the buffer is now `new_len` bytes long. Re-scans
    /// from the chunk holding the old last byte (whose look-ahead may have
    /// changed) to the end.
    pub fn extend(&self, source: &dyn Source, new_len: u64) -> io::Result<()> {
        let lo = {
            let mut st = self.state.write();
            let old_len = st.len;
            if new_len <= old_len {
                return Ok(());
            }
            let n = State::chunks(new_len);
            st.len = new_len;
            st.counts.resize(n, UNKNOWN);
            st.stats.resize(n, EndingStats::default());
            st.fingerprints.resize(n, 0);
            st.prefix = None;
            (old_len.saturating_sub(1) / CHUNK) as usize
        };
        let hi = self.chunk_count();
        self.scan_range(source, lo, hi, None)
    }

    /// Break positions in `a..b` of the buffer, if the chunks it needs are
    /// indexed. Partial chunks are scanned from `source`.
    pub fn breakpos_in(&self, source: &dyn Source, a: u64, b: u64) -> io::Result<Option<u64>> {
        if a >= b {
            return Ok(Some(0));
        }
        let (ca, cb) = ((a / CHUNK) as usize, ((b - 1) / CHUNK) as usize);
        let len = self.len();
        // Whole chunks strictly between the partial ends.
        let a_aligned = a.is_multiple_of(CHUNK);
        let b_aligned = b.is_multiple_of(CHUNK) || b == len;
        if ca == cb {
            if a_aligned && b_aligned {
                return Ok(self.state.read().sum(ca, ca + 1));
            }
            return scan_span(source, a, b).map(Some);
        }
        let first_full = if a_aligned { ca } else { ca + 1 };
        let last_full = if b_aligned { cb + 1 } else { cb };
        let Some(mid) = self.state.read().sum(first_full, last_full) else {
            return Ok(None);
        };
        let mut total = mid;
        if !a_aligned {
            total += scan_span(source, a, (ca as u64 + 1) * CHUNK)?;
        }
        if !b_aligned {
            total += scan_span(source, cb as u64 * CHUNK, b)?;
        }
        Ok(Some(total))
    }

    /// Breaks in a piece `a..b` of this buffer: break positions, plus one if
    /// the piece ends on a CR whose LF is outside the piece (inside the
    /// piece, that CR is the last byte of a line).
    pub fn piece_breaks(&self, source: &dyn Source, a: u64, b: u64) -> io::Result<Option<u64>> {
        let Some(n) = self.breakpos_in(source, a, b)? else {
            return Ok(None);
        };
        Ok(Some(n + u64::from(split_crlf_at(source, a, b)?)))
    }

    /// Offset just past the `k`-th (1-based) break of piece `a..b`, if the
    /// chunks it needs are indexed. `Ok(Some(None))`: the piece has fewer
    /// than `k` breaks.
    pub fn piece_nth_break(
        &self,
        source: &dyn Source,
        a: u64,
        b: u64,
        k: u64,
    ) -> io::Result<Option<Option<u64>>> {
        debug_assert!(k >= 1);
        let len = self.len();
        let mut remaining = k;
        let mut at = a;
        while at < b {
            // Whole indexed chunks are skipped by their counts: a binary
            // search over the prefix sums once the index is complete.
            if at.is_multiple_of(CHUNK) {
                let c = (at / CHUNK) as usize;
                let last = if b == len {
                    len.div_ceil(CHUNK) as usize
                } else {
                    (b / CHUNK) as usize
                };
                if last > c {
                    let Some((j, n)) = self.state.read().skip(c, last, remaining) else {
                        return Ok(None);
                    };
                    remaining -= n;
                    at = (j as u64 * CHUNK).min(len);
                    if j < last {
                        // The target is inside chunk j.
                        let end = ((j as u64 + 1) * CHUNK).min(len);
                        let (_, hit) = scan_nth(source, at, end, remaining, len)?;
                        return Ok(Some(hit));
                    }
                    continue;
                }
            }
            let chunk_end = ((at / CHUNK + 1) * CHUNK).min(len);
            let seg_end = chunk_end.min(b);
            let (n, hit) = scan_nth(source, at, seg_end, remaining, len)?;
            if hit.is_some() {
                return Ok(Some(hit));
            }
            remaining -= n;
            at = seg_end;
        }
        // The piece-end CR of a split CRLF is one more break.
        if remaining == 1 && split_crlf_at(source, a, b)? {
            return Ok(Some(Some(b)));
        }
        Ok(Some(None))
    }

    #[cfg(test)]
    fn known_chunks(&self) -> usize {
        self.state.read().known
    }
}

/// Whether piece `a..b` ends on a CR that the buffer follows with an LF.
fn split_crlf_at(source: &dyn Source, a: u64, b: u64) -> io::Result<bool> {
    if b <= a || b >= source.len() {
        return Ok(false);
    }
    let mut two = [0u8; 2];
    source.read_at(b - 1, &mut two)?;
    Ok(two == *b"\r\n")
}

/// Break positions in `a..b`, with the buffer's byte at `b` as look-ahead.
fn scan_span(source: &dyn Source, a: u64, b: u64) -> io::Result<u64> {
    let read_end = (b + 1).min(source.len());
    let mut buf = vec![0u8; (read_end - a) as usize];
    source.read_at(a, &mut buf)?;
    let (bytes, next) = buf.split_at((b - a) as usize);
    Ok(scan(bytes, None, next.first().copied()).0)
}

/// Scans `a..b` for the `k`-th break position. Returns the number of break
/// positions seen (when not found) and the hit, if any.
fn scan_nth(
    source: &dyn Source,
    a: u64,
    b: u64,
    k: u64,
    len: u64,
) -> io::Result<(u64, Option<u64>)> {
    let read_end = (b + 1).min(len);
    let mut buf = vec![0u8; (read_end - a) as usize];
    source.read_at(a, &mut buf)?;
    let (bytes, next) = buf.split_at((b - a) as usize);
    let next = next.first().copied();
    if let Some(p) = nth_break(bytes, next, k) {
        return Ok((k, Some(a + p as u64)));
    }
    Ok((scan(bytes, None, next).0, None))
}

/// The longest an index job waits for foreground reads before it goes on.
pub const MAX_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

/// Indexes `source` in the background on `scheduler`, in jobs of 32 MiB
/// submitted front to back, each prefetching a short window ahead of its
/// reads (ADR-0007 addendum: a deep prefetch queue made foreground reads
/// wait). While `foreground` is non-zero (a view is being built from the
/// same file), jobs pause between blocks, for at most [`MAX_PAUSE`] each
/// time. `on_progress` runs after each job.
pub fn index_in_background(
    scheduler: &quire_core::scheduler::Scheduler,
    source: Arc<dyn Source>,
    index: Arc<ChunkIndex>,
    cancel: quire_core::scheduler::CancelToken,
    foreground: Arc<std::sync::atomic::AtomicUsize>,
    on_progress: Arc<dyn Fn(&ChunkIndex) + Send + Sync>,
) -> Vec<quire_core::scheduler::JobHandle<()>> {
    use quire_core::scheduler::{JobSpec, Priority};
    const JOB_CHUNKS: usize = 512; // 32 MiB
    const AHEAD: u64 = 8 << 20;
    let n = index.chunk_count();
    let mut handles = Vec::new();
    let mut lo = 0;
    while lo < n {
        let hi = (lo + JOB_CHUNKS).min(n);
        let (source, index, cancel, foreground, on_progress) = (
            Arc::clone(&source),
            Arc::clone(&index),
            cancel.clone(),
            Arc::clone(&foreground),
            Arc::clone(&on_progress),
        );
        handles.push(scheduler.submit(
            JobSpec::new("txt-index", Priority::Background),
            move |ctx| {
                let stop = || {
                    let paused = std::time::Instant::now();
                    while foreground.load(Ordering::Acquire) > 0 && paused.elapsed() < MAX_PAUSE {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    cancel.is_cancelled() || ctx.is_cancelled()
                };
                if let Err(err) = index.scan_range_ahead(&*source, lo, hi, Some(&stop), AHEAD) {
                    tracing::warn!(%err, chunk = lo, "line index read failed");
                }
                on_progress(&index);
                Ok(())
            },
        ));
        lo = hi;
    }
    handles
}

#[cfg(test)]
mod tests;
