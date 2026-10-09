//! The document's bytes: a piece tree over a table of buffers (§6.1).
//!
//! - Buffer 0 is the original (mapped, or transcoded to UTF-8).
//! - Buffer 1 is the append-only add buffer: every inserted byte is appended
//!   there once and never moves.
//! - Later buffers are derived content (a replace-all result) or history
//!   spill (undo ranges copied out of an old mapping before a save, §5.8).
//!
//! [`TextBuffer`] owns the current tree and applies edits; a [`Snapshot`] is
//! an immutable version that background jobs (search, save, index refresh)
//! read while editing continues.

use std::io;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::index::ChunkIndex;
use crate::source::{Source, SourceKind, check_range};
use crate::tree::{BufferId, Piece, PieceContext, PieceTree};

pub const ORIGINAL: BufferId = 0;
pub const ADD: BufferId = 1;
/// Line bounds are first looked for this close, before using the index.
const SHORT_SCAN: u64 = 4096;

/// Append-only bytes in memory, in fixed blocks so appends never move data.
pub struct AddBuffer {
    inner: RwLock<AddInner>,
}

struct AddInner {
    blocks: Vec<Vec<u8>>,
    len: u64,
}

const BLOCK: usize = 1 << 20;

impl std::fmt::Debug for AddBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddBuffer")
            .field("len", &self.len())
            .finish()
    }
}

impl Default for AddBuffer {
    fn default() -> Self {
        Self {
            inner: RwLock::new(AddInner {
                blocks: Vec::new(),
                len: 0,
            }),
        }
    }
}

impl AddBuffer {
    /// Appends `bytes`; returns where they start.
    pub fn append(&self, mut bytes: &[u8]) -> u64 {
        let mut st = self.inner.write();
        let start = st.len;
        while !bytes.is_empty() {
            if st.blocks.last().is_none_or(|b| b.len() == BLOCK) {
                st.blocks.push(Vec::with_capacity(BLOCK));
            }
            let last = st.blocks.last_mut().expect("just pushed");
            let n = (BLOCK - last.len()).min(bytes.len());
            last.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            st.len += n as u64;
        }
        start
    }
}

impl Source for AddBuffer {
    fn len(&self) -> u64 {
        self.inner.read().len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let st = self.inner.read();
        check_range(offset, buf.len(), st.len)?;
        let mut done = 0;
        while done < buf.len() {
            let at = offset as usize + done;
            let block = &st.blocks[at / BLOCK];
            let within = at % BLOCK;
            let n = (block.len() - within).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&block[within..within + n]);
            done += n;
        }
        Ok(())
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Memory
    }
}

/// A buffer and its line index.
#[derive(Clone, Debug)]
pub struct BufferEntry {
    pub source: Arc<dyn Source>,
    pub index: Arc<ChunkIndex>,
}

/// The table of buffers. Ids are indices and stay valid; the table only grows.
#[derive(Clone, Debug, Default)]
pub struct Buffers {
    entries: Arc<RwLock<Vec<BufferEntry>>>,
}

impl Buffers {
    pub fn get(&self, id: BufferId) -> BufferEntry {
        self.entries.read()[id as usize].clone()
    }

    pub fn push(&self, entry: BufferEntry) -> BufferId {
        let mut e = self.entries.write();
        e.push(entry);
        (e.len() - 1) as BufferId
    }

    /// Replaces buffer `id` (after a save remaps the original, §5.8).
    pub fn replace(&self, id: BufferId, entry: BufferEntry) {
        self.entries.write()[id as usize] = entry;
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_piece(&self, p: &Piece, within: u64, buf: &mut [u8]) -> io::Result<()> {
        self.get(p.buf).source.read_at(p.start + within, buf)
    }
}

impl PieceContext for Buffers {
    fn breaks(&self, buf: BufferId, start: u64, len: u64) -> Option<u64> {
        let e = self.get(buf);
        e.index
            .piece_breaks(&*e.source, start, start + len)
            .ok()
            .flatten()
    }

    fn nth_break(&self, buf: BufferId, start: u64, len: u64, k: u64) -> Option<Option<u64>> {
        let e = self.get(buf);
        e.index
            .piece_nth_break(&*e.source, start, start + len, k)
            .ok()
            .flatten()
    }
}

/// Receives `(offset, bytes)` blocks; breaks to stop.
pub type BlockFn<'a> = dyn FnMut(u64, &[u8]) -> io::Result<ControlFlow<()>> + 'a;

/// An immutable version of the document.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tree: PieceTree,
    pub buffers: Buffers,
}

/// A line number that may be an estimate (§7: label estimates as such).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineNumber {
    pub line: u64,
    pub exact: bool,
}

impl Snapshot {
    pub fn len(&self) -> u64 {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads `range` into a new vector.
    pub fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        let end = range.end.min(self.len());
        let start = range.start.min(end);
        let mut out = vec![0u8; (end - start) as usize];
        self.read_into(start, &mut out)?;
        Ok(out)
    }

    /// Fills `buf` with the bytes at `offset`.
    pub fn read_into(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(offset, buf.len(), self.len())?;
        let end = offset + buf.len() as u64;
        let mut result = Ok(());
        let _ = self.tree.visit(offset, end, &mut |ps, p| {
            let a = offset.max(ps);
            let b = end.min(ps + p.len);
            let dst = &mut buf[(a - offset) as usize..(b - offset) as usize];
            match self.buffers.read_piece(p, a - ps, dst) {
                Ok(()) => ControlFlow::Continue(()),
                Err(e) => {
                    result = Err(e);
                    ControlFlow::Break(())
                }
            }
        });
        result
    }

    pub fn byte_at(&self, offset: u64) -> io::Result<u8> {
        let mut b = [0u8];
        self.read_into(offset, &mut b)?;
        Ok(b[0])
    }

    /// Starts reading `range` ahead (mapped sources queue the reads; see
    /// ADR-0007).
    pub fn prefetch(&self, range: Range<u64>) {
        let _ = self.tree.visit(range.start, range.end, &mut |ps, p| {
            let a = range.start.max(ps);
            let b = range.end.min(ps + p.len);
            self.buffers
                .get(p.buf)
                .source
                .prefetch(p.start + (a - ps), b - a);
            ControlFlow::Continue(())
        });
    }

    /// Position of the first (or, with `last`, the last) CR or LF in
    /// `range`. Before the line index reaches a long line, finding its end
    /// or start is a cold scan of up to the whole file, so large ranges are
    /// read ahead and split across threads, a stage at a time from the near
    /// end.
    fn find_cr_lf(&self, range: Range<u64>, last: bool) -> io::Result<Option<u64>> {
        const BLOCK: u64 = 1 << 20;
        const STAGE: u64 = 32 << 20;
        const PARTS: u64 = 4;
        let scan = |r: Range<u64>| -> io::Result<Option<u64>> {
            let mut buf = vec![0u8; BLOCK.min(r.end - r.start) as usize];
            let (mut lo, mut hi) = (r.start, r.end);
            while lo < hi {
                let n = BLOCK.min(hi - lo);
                let at = if last { hi - n } else { lo };
                let bytes = &mut buf[..n as usize];
                self.read_into(at, bytes)?;
                let hit = if last {
                    memchr::memrchr2(b'\n', b'\r', bytes)
                } else {
                    memchr::memchr2(b'\n', b'\r', bytes)
                };
                if let Some(p) = hit {
                    return Ok(Some(at + p as u64));
                }
                if last {
                    hi = at;
                } else {
                    lo = at + n;
                }
            }
            Ok(None)
        };
        let (mut lo, mut hi) = (range.start, range.end.min(self.len()));
        while lo < hi {
            let (a, b) = if last {
                (hi.saturating_sub(STAGE).max(lo), hi)
            } else {
                (lo, (lo + STAGE).min(hi))
            };
            let found = if b - a <= 2 * BLOCK {
                scan(a..b)?
            } else {
                self.prefetch(a..b);
                let part = (b - a).div_ceil(PARTS);
                let results: Vec<io::Result<Option<u64>>> = std::thread::scope(|s| {
                    let handles: Vec<_> = (0..PARTS)
                        .map(|i| {
                            let pa = (a + i * part).min(b);
                            let pb = (pa + part).min(b);
                            let scan = &scan;
                            s.spawn(move || if pa < pb { scan(pa..pb) } else { Ok(None) })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| h.join().expect("scan thread panicked"))
                        .collect()
                });
                let mut hits = Vec::new();
                for r in results {
                    hits.extend(r?);
                }
                if last {
                    hits.into_iter().max()
                } else {
                    hits.into_iter().min()
                }
            };
            if found.is_some() {
                return Ok(found);
            }
            if last {
                hi = a;
            } else {
                lo = b;
            }
        }
        Ok(None)
    }

    /// Calls `f(offset, bytes)` over `range` in blocks of at most `block`
    /// bytes, in order, until it breaks.
    pub fn for_each_block(
        &self,
        range: Range<u64>,
        block: usize,
        f: &mut BlockFn<'_>,
    ) -> io::Result<()> {
        let end = range.end.min(self.len());
        let mut at = range.start.min(end);
        let mut buf = vec![0u8; block.min((end - at) as usize)];
        while at < end {
            let n = (block as u64).min(end - at) as usize;
            self.read_into(at, &mut buf[..n])?;
            if f(at, &buf[..n])?.is_break() {
                break;
            }
            at += n as u64;
        }
        Ok(())
    }

    /// Exact line of `offset` (number of breaks before it), if indexed.
    pub fn line_of(&self, offset: u64) -> Option<u64> {
        self.tree.breaks_before(offset, &self.buffers)
    }

    /// Exact start of line `line`, if indexed. `Some(None)`: no such line.
    pub fn line_start(&self, line: u64) -> Option<Option<u64>> {
        self.tree.line_start(line, &self.buffers)
    }

    /// Exact number of lines, once everything is indexed.
    pub fn line_count(&self) -> Option<u64> {
        let s = self.tree.summary();
        s.is_exact().then_some(s.breaks + 1)
    }

    /// Breaks per byte, from what is indexed (for estimates).
    pub fn density(&self) -> f64 {
        let s = self.tree.summary();
        let known = s.bytes - s.unknown;
        if known >= 64 * 1024 {
            return s.breaks as f64 / known as f64;
        }
        if let Some(d) = self.buffers.get(ORIGINAL).index.density()
            && d > 0.0
        {
            return d;
        }
        // Nothing indexed yet: sample the start of the document.
        let n = self.len().min(64 * 1024);
        match self.read(0..n) {
            Ok(head) if n > 0 => (crate::index::scan(&head, None, None).0.max(1)) as f64 / n as f64,
            _ => 1.0 / 80.0,
        }
    }

    /// Line count, estimated from the indexed part until indexing finishes.
    pub fn line_count_estimate(&self) -> LineNumber {
        match self.line_count() {
            Some(n) => LineNumber {
                line: n,
                exact: true,
            },
            None => {
                let s = self.tree.summary();
                LineNumber {
                    line: s.breaks + (s.unknown as f64 * self.density()).round() as u64 + 1,
                    exact: false,
                }
            }
        }
    }

    /// Line of `offset`, exact if possible, else proportional to the byte
    /// offset within the unindexed part (§6.1).
    pub fn line_of_estimate(&self, offset: u64) -> LineNumber {
        if let Some(line) = self.line_of(offset) {
            return LineNumber { line, exact: true };
        }
        let d = self.density();
        let mut known = 0u64;
        let mut unknown = 0u64;
        let _ = self.tree.visit(0, offset, &mut |ps, p| {
            let take = (offset - ps).min(p.len);
            match (take == p.len, p.breaks) {
                (true, Some(b)) => known += b,
                _ => match self.buffers.breaks(p.buf, p.start, take) {
                    Some(b) => known += b,
                    None => unknown += take,
                },
            }
            ControlFlow::Continue(())
        });
        LineNumber {
            line: known + (unknown as f64 * d) as u64,
            exact: false,
        }
    }

    /// Start of line `line`: exact if possible, otherwise an offset found
    /// proportionally to bytes in the unindexed part and moved to the start
    /// of the line it falls in. Clamped to the last line.
    pub fn line_start_estimate(&self, line: u64) -> (u64, bool) {
        match self.line_start(line) {
            Some(Some(off)) => return (off, true),
            Some(None) => {
                let last = self.line_count().unwrap_or(1) - 1;
                if let Some(Some(off)) = self.line_start(last) {
                    return (off, true);
                }
            }
            None => {}
        }
        let d = self.density().max(1e-9);
        let mut remaining = line as f64;
        let mut target = None;
        let _ = self.tree.visit(0, u64::MAX, &mut |ps, p| {
            if remaining <= 0.0 {
                target = Some(ps);
                return ControlFlow::Break(());
            }
            match p.breaks {
                Some(b) if remaining <= b as f64 => {
                    let k = (remaining.round() as u64).max(1);
                    target = Some(match self.buffers.nth_break(p.buf, p.start, p.len, k) {
                        Some(Some(at)) => ps + (at - p.start),
                        _ => ps,
                    });
                    ControlFlow::Break(())
                }
                Some(b) => {
                    remaining -= b as f64;
                    ControlFlow::Continue(())
                }
                None => {
                    let est = p.len as f64 * d;
                    if remaining <= est {
                        target = Some(ps + ((remaining / d) as u64).min(p.len.saturating_sub(1)));
                        ControlFlow::Break(())
                    } else {
                        remaining -= est;
                        ControlFlow::Continue(())
                    }
                }
            }
        });
        let target = target.unwrap_or(self.len());
        (self.line_start_containing(target).unwrap_or(target), false)
    }

    /// Start of the line holding `offset`. Ordinary lines are found by a
    /// short scan back; long ones through the index once the lines before
    /// them are indexed, else by scanning all the way.
    pub fn line_start_containing(&self, offset: u64) -> io::Result<u64> {
        let offset = offset.min(self.len());
        if let Some(start) = self.scan_back_for_line_start(offset, Some(SHORT_SCAN))? {
            return Ok(start);
        }
        if let Some(line) = self.line_of(offset)
            && let Some(Some(start)) = self.line_start(line)
        {
            return Ok(start);
        }
        Ok(self.scan_back_for_line_start(offset, None)?.unwrap_or(0))
    }

    /// Scans back from `offset` for the start of its line, at most `limit`
    /// bytes (`None`: no limit). `Ok(None)`: not within the limit.
    fn scan_back_for_line_start(&self, offset: u64, limit: Option<u64>) -> io::Result<Option<u64>> {
        const STEP: u64 = 64 * 1024;
        if limit.is_none() {
            // The last break strictly before `offset`; a CR whose LF is at
            // or past `offset` isn't one (the LF ends that line).
            let mut end = offset;
            while let Some(p) = self.find_cr_lf(0..end, true)? {
                let is_break = self.byte_at(p)? == b'\n'
                    || p + 1 >= self.len()
                    || self.byte_at(p + 1)? != b'\n';
                if is_break {
                    return Ok(Some(p + 1));
                }
                end = p;
            }
            return Ok(Some(0));
        }
        let floor = limit.map_or(0, |l| offset.saturating_sub(l));
        let mut end = offset;
        // A break at offset-1 ends the previous line: `offset` starts one.
        while end > floor {
            let start = end.saturating_sub(STEP).max(floor);
            let bytes = self.read(start..end)?;
            // Look for the last break position strictly before `offset`.
            let mut i = bytes.len();
            while let Some(p) = memchr::memrchr2(b'\n', b'\r', &bytes[..i]) {
                let at = start + p as u64;
                let is_break = bytes[p] == b'\n' || {
                    let next = if p + 1 < bytes.len() {
                        Some(bytes[p + 1])
                    } else if at + 1 < self.len() {
                        Some(self.byte_at(at + 1)?)
                    } else {
                        None
                    };
                    next != Some(b'\n')
                };
                if is_break && at < offset {
                    return Ok(Some(at + 1));
                }
                i = p;
            }
            end = start;
        }
        Ok((floor == 0).then_some(0))
    }

    /// End of the line holding `offset`, excluding its break bytes, and the
    /// offset where the next line starts (= len at the last line). Short
    /// scan first, then the index, then a full scan.
    pub fn line_end(&self, offset: u64) -> io::Result<(u64, u64)> {
        let len = self.len();
        let offset = offset.min(len);
        if let Some(r) = self.scan_line_end(offset, Some(SHORT_SCAN))? {
            return Ok(r);
        }
        // Through the index: the next line's start, minus its break bytes.
        if let Some(line) = self.line_of(offset) {
            match self.line_start(line + 1) {
                Some(Some(next)) => {
                    let crlf = next >= 2 && self.read(next - 2..next)? == b"\r\n";
                    return Ok((next - if crlf { 2 } else { 1 }, next));
                }
                Some(None) => return Ok((len, len)),
                None => {}
            }
        }
        Ok(self.scan_line_end(offset, None)?.unwrap_or((len, len)))
    }

    fn scan_line_end(&self, offset: u64, limit: Option<u64>) -> io::Result<Option<(u64, u64)>> {
        const STEP: u64 = 64 * 1024;
        let len = self.len();
        if limit.is_none() {
            let Some(pos) = self.find_cr_lf(offset..len, false)? else {
                return Ok(Some((len, len)));
            };
            let crlf =
                self.byte_at(pos)? == b'\r' && pos + 1 < len && self.byte_at(pos + 1)? == b'\n';
            return Ok(Some((pos, pos + if crlf { 2 } else { 1 })));
        }
        let ceil = limit.map_or(len, |l| (offset + l).min(len));
        let mut at = offset;
        while at < ceil {
            let end = (at + STEP).min(ceil);
            let bytes = self.read(at..end)?;
            if let Some(p) = memchr::memchr2(b'\n', b'\r', &bytes) {
                let pos = at + p as u64;
                if bytes[p] == b'\r' {
                    let next = if p + 1 < bytes.len() {
                        Some(bytes[p + 1])
                    } else if pos + 1 < len {
                        Some(self.byte_at(pos + 1)?)
                    } else {
                        None
                    };
                    let skip = if next == Some(b'\n') { 2 } else { 1 };
                    return Ok(Some((pos, pos + skip)));
                }
                return Ok(Some((pos, pos + 1)));
            }
            at = end;
        }
        Ok((ceil == len).then_some((len, len)))
    }
}

/// Appends to a document's add buffer.
#[derive(Clone, Debug)]
pub struct Adder {
    add: Arc<AddBuffer>,
    index: Arc<ChunkIndex>,
    pub buffers: Buffers,
}

impl Adder {
    pub fn append(&self, bytes: &[u8]) -> io::Result<Piece> {
        let start = self.add.append(bytes);
        self.index.extend(&*self.add, self.add.len())?;
        Ok(Piece {
            buf: ADD,
            start,
            len: bytes.len() as u64,
            breaks: self.buffers.breaks(ADD, start, bytes.len() as u64),
        })
    }
}

/// What an edit removed, for its inverse.
#[derive(Clone, Debug)]
pub struct Removed {
    pub at: u64,
    pub pieces: PieceTree,
    /// What took its place.
    pub inserted: PieceTree,
}

/// The current document: a tree over the buffer table, plus the add buffer.
#[derive(Debug)]
pub struct TextBuffer {
    buffers: Buffers,
    tree: PieceTree,
    add: Arc<AddBuffer>,
    add_index: Arc<ChunkIndex>,
}

impl TextBuffer {
    /// A document over `original` (buffer 0), indexed by `index`, which may
    /// still be filling in the background.
    pub fn new(original: Arc<dyn Source>, index: Arc<ChunkIndex>) -> Self {
        Self::with_original(original, index, 0)
    }

    /// Like [`TextBuffer::new`], with the document starting at `start` of
    /// the original (after a byte-order mark).
    pub fn with_original(original: Arc<dyn Source>, index: Arc<ChunkIndex>, start: u64) -> Self {
        let buffers = Buffers::default();
        let len = original.len() - start;
        let breaks = index
            .piece_breaks(&*original, start, start + len)
            .ok()
            .flatten();
        buffers.push(BufferEntry {
            source: original,
            index,
        });
        let add = Arc::new(AddBuffer::default());
        let add_index = Arc::new(ChunkIndex::unindexed(0));
        buffers.push(BufferEntry {
            source: Arc::clone(&add) as Arc<dyn Source>,
            index: Arc::clone(&add_index),
        });
        let tree = PieceTree::from_pieces(&[Piece {
            buf: ORIGINAL,
            start,
            len,
            breaks,
        }]);
        Self {
            buffers,
            tree,
            add,
            add_index,
        }
    }

    /// Points the document at a new original (after a save, §5.8 step 5):
    /// buffer 0 becomes `entry` and the tree one piece over it from `start`.
    pub fn rebase(&mut self, entry: BufferEntry, start: u64) {
        let len = entry.source.len() - start;
        let breaks = entry
            .index
            .piece_breaks(&*entry.source, start, start + len)
            .ok()
            .flatten();
        self.buffers.replace(ORIGINAL, entry);
        self.tree = PieceTree::from_pieces(&[Piece {
            buf: ORIGINAL,
            start,
            len,
            breaks,
        }]);
    }

    /// A document holding `bytes` in memory (new files, tests).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let src: Arc<dyn Source> = Arc::new(crate::source::MemorySource::new(bytes.to_vec()));
        let index = Arc::new(ChunkIndex::build(&*src).expect("memory reads don't fail"));
        Self::new(src, index)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tree: self.tree.clone(),
            buffers: self.buffers.clone(),
        }
    }

    pub fn tree(&self) -> &PieceTree {
        &self.tree
    }

    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    pub fn len(&self) -> u64 {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes held by the add buffer (counted against the memory budget).
    pub fn add_bytes(&self) -> u64 {
        self.add.len()
    }

    /// Appends `bytes` to the add buffer and returns them as a piece.
    pub fn append(&self, bytes: &[u8]) -> io::Result<Piece> {
        self.adder().append(bytes)
    }

    /// A handle that appends to the add buffer from another thread
    /// (replace-all builds its result off the document lock).
    pub fn adder(&self) -> Adder {
        Adder {
            add: Arc::clone(&self.add),
            index: Arc::clone(&self.add_index),
            buffers: self.buffers.clone(),
        }
    }

    /// Replaces `from..to` with `text`. Returns what was removed.
    pub fn replace(&mut self, from: u64, to: u64, text: &[u8]) -> io::Result<Removed> {
        let to = to.min(self.len());
        let from = from.min(to);
        let (rest, removed) = self.tree.remove(from, to, &self.buffers);
        self.tree = rest;
        let mut inserted = PieceTree::new();
        if !text.is_empty() {
            let piece = self.append(text)?;
            inserted = PieceTree::from_pieces(&[piece]);
            self.tree = match self.tree.extend_at(from, &piece, &self.buffers) {
                Some(t) => t,
                None => self
                    .tree
                    .insert(from, &PieceTree::from_pieces(&[piece]), &self.buffers),
            };
        }
        self.fix_junctions(from, from + text.len() as u64)?;
        Ok(Removed {
            at: from,
            pieces: removed,
            inserted,
        })
    }

    /// Replaces `from..to` with existing pieces (undo and redo).
    pub fn replace_pieces(&mut self, from: u64, to: u64, pieces: &PieceTree) -> io::Result<()> {
        let (rest, _) = self.tree.remove(from, to, &self.buffers);
        self.tree = rest.insert(from, pieces, &self.buffers);
        self.fix_junctions(from, from + pieces.len())
    }

    /// Inserts existing pieces at `at` (undo of a removal).
    pub fn insert_pieces(&mut self, at: u64, pieces: &PieceTree) -> io::Result<()> {
        self.tree = self.tree.insert(at, pieces, &self.buffers);
        self.fix_junctions(at, at + pieces.len())
    }

    /// Removes `from..to`, returning the pieces (undo of an insertion).
    pub fn remove(&mut self, from: u64, to: u64) -> io::Result<Removed> {
        self.replace(from, to, &[])
    }

    /// Replaces the whole tree (replace-all, its undo, reload after save).
    pub fn set_tree(&mut self, tree: PieceTree) {
        self.tree = tree;
    }

    pub fn push_buffer(&self, entry: BufferEntry) -> BufferId {
        self.buffers.push(entry)
    }

    /// Re-reads break counts that were unknown (call when the index advanced).
    pub fn refresh_index(&mut self) {
        if !self.tree.summary().is_exact() {
            self.tree = self.tree.refresh_unknown(&self.buffers);
        }
    }

    /// A CRLF whose CR ends one piece and whose LF starts the next would be
    /// counted as two breaks. After an edit, such pairs at the edit's two
    /// junctions are rejoined into one piece holding both bytes.
    fn fix_junctions(&mut self, a: u64, b: u64) -> io::Result<()> {
        for j in [b, a] {
            if j == 0 || j >= self.len() {
                continue;
            }
            let snap = self.snapshot();
            let mut two = [0u8; 2];
            snap.read_into(j - 1, &mut two)?;
            if two != *b"\r\n" {
                continue;
            }
            let at_boundary = self.tree.piece_at(j).is_some_and(|(start, _)| start == j);
            if !at_boundary {
                continue;
            }
            let (rest, _) = self.tree.remove(j - 1, j + 1, &self.buffers);
            let piece = self.append(b"\r\n")?;
            self.tree = rest.insert(j - 1, &PieceTree::from_pieces(&[piece]), &self.buffers);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
