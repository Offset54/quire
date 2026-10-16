//! The top-level block index (ADR-0027).
//!
//! Built front to back in stretches ([`BlockIndex::extend`]), so the blocks
//! around the first viewport exist after one small parse, and the rest is
//! indexed in the background. The indexed part ends at the *frontier*, a
//! position where a top-level block starts.
//!
//! An edit re-scans from the start of the block before the one it touches (an
//! edit can merge into the previous block: a setext underline, a lazy
//! continuation line, a removed blank line) and stops as soon as a scanned
//! block starts, after the edit, exactly where an old block started (shifted
//! by the edit's length change). At such a position the parser's state is the
//! same as before (no open container), so everything after it is unchanged.
//! If no such position comes within the edit budget, the index is cut at the
//! new frontier and the background indexing continues from there.

use std::io;
use std::ops::Range;

use xxhash_rust::xxh3::xxh3_64;

use crate::options::Flavor;
use crate::scan::{BlockKind, FRONT_MATTER_MAX, front_matter_end, front_matter_end_within, scan};
use crate::text::{Text, mid_document};

/// One top-level block: whole lines `start..end` (including the last line's
/// break). Blank lines between blocks belong to none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Stable across edits that don't touch the block.
    pub id: u64,
    pub start: u64,
    pub end: u64,
    pub kind: BlockKind,
    /// Hash of the block's bytes (render and measurement caches key on it).
    pub hash: u64,
    /// A paragraph starting with `[`: an edit to it or after it can make it
    /// link reference definitions only, and then no block starts here.
    pub may_vanish: bool,
}

impl Block {
    pub fn range(&self) -> Range<u64> {
        self.start..self.end
    }
}

/// What an edit changed in the index: the blocks at `first..first + inserted`
/// replace the removed ids; blocks after them only moved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub first: usize,
    pub inserted: usize,
    pub removed: Vec<u64>,
    /// Ids of the old blocks at `first..`, in order, that the new ones
    /// replace (reused ids included).
    pub replaced: Vec<u64>,
    /// Bytes re-parsed to apply the edit.
    pub scanned: u64,
    /// The edit's re-scan didn't line up with the old blocks within its
    /// budget: the index now ends at the frontier and needs extending.
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct BlockIndex {
    blocks: Vec<Block>,
    /// Bytes before this are indexed; a block starts here (or it is `len`).
    frontier: u64,
    len: u64,
    next_id: u64,
    /// Bytes per scanned stretch.
    chunk: usize,
    flavor: Flavor,
}

/// Default stretch: large enough for the first viewport of typical text, and
/// about 2 ms of parsing.
pub const CHUNK: usize = 64 * 1024;

/// Bytes an edit may re-parse before it leaves the rest to the background.
pub const EDIT_BUDGET: u64 = 128 * 1024;

/// Stretch size for an edit's re-scan.
const EDIT_CHUNK: usize = 4 * 1024;

/// Where a stretch starting at `from` ends: `from + chunk` moved to the end
/// of that line, or the end of the text.
fn stretch_end<T: Text + ?Sized>(text: &T, from: u64, chunk: usize) -> io::Result<u64> {
    let len = text.len();
    let mut end = from.saturating_add(chunk as u64);
    if end >= len {
        return Ok(len);
    }
    // Extend to the end of the line (a block can't be cut mid-line).
    loop {
        let probe = text.read(end..(end + 4096).min(len))?;
        if let Some(i) = probe.iter().position(|&b| b == b'\n') {
            return Ok(end + i as u64 + 1);
        }
        end += probe.len() as u64;
        if end >= len {
            return Ok(len);
        }
    }
}

/// A block from a scan, before it has an id.
struct Scanned {
    range: Range<u64>,
    kind: BlockKind,
    hash: u64,
    may_vanish: bool,
}

impl Scanned {
    fn into_block(self, id: u64) -> Block {
        Block {
            id,
            start: self.range.start,
            end: self.range.end,
            kind: self.kind,
            hash: self.hash,
            may_vanish: self.may_vanish,
        }
    }
}

/// Whether `bytes` contain a whole blank line after their first line.
fn has_blank_line(bytes: &[u8]) -> bool {
    let starts = crate::text::line_starts(bytes);
    starts.windows(2).skip(1).any(|w| {
        bytes[w[0]..w[1]]
            .iter()
            .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    })
}

impl BlockIndex {
    /// An empty index for a text of `len` bytes.
    pub fn new(len: u64) -> Self {
        Self::with_chunk(len, CHUNK)
    }

    /// As [`new`](Self::new), scanning `chunk` bytes per stretch.
    pub fn with_chunk(len: u64, chunk: usize) -> Self {
        Self::with_flavor(len, chunk, Flavor::PRODUCT)
    }

    /// An index of Markdown with the given extensions.
    pub fn with_flavor(len: u64, chunk: usize, flavor: Flavor) -> Self {
        Self {
            blocks: Vec::new(),
            frontier: 0,
            len,
            next_id: 1,
            chunk: chunk.max(16),
            flavor,
        }
    }

    pub fn flavor(&self) -> &Flavor {
        &self.flavor
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// End of the indexed part.
    pub fn frontier(&self) -> u64 {
        self.frontier
    }

    pub fn is_complete(&self) -> bool {
        self.frontier >= self.len
    }

    /// Length of the text the index describes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Scans one stretch at `from`: the final blocks in it (absolute ranges,
    /// with their bytes' hashes) and the position after them, where the next
    /// stretch starts. Grows the stretch while it holds fewer than two blocks
    /// (one huge block), up to `limit` bytes: past that it returns no blocks
    /// and `from` (the block is still open).
    fn scan_stretch<T: Text + ?Sized>(
        &self,
        text: &T,
        from: u64,
        chunk: usize,
        limit: u64,
    ) -> io::Result<(Vec<Scanned>, u64)> {
        let len = text.len();
        if from == 0
            && self.flavor.front_matter
            && let Some(end) = front_matter_end(text)?
        {
            let hash = xxh3_64(&text.read(0..end)?);
            let fm = Scanned {
                range: 0..end,
                kind: BlockKind::FrontMatter,
                hash,
                may_vanish: false,
            };
            return Ok((vec![fm], end));
        }
        let mut chunk = chunk;
        loop {
            let end = stretch_end(text, from, chunk)?;
            let bytes = text.read(from..end)?;
            let mut found = scan(&mid_document(&bytes, from), &self.flavor);
            let at_end = end >= len;
            let next = if at_end {
                len
            } else {
                // The last block may continue past the stretch, so it is
                // scanned again. If it may still vanish, the block before it
                // isn't settled either: its range runs to the last one's start.
                let keep_back = if found.last().is_some_and(|f| f.may_vanish) {
                    2
                } else {
                    1
                };
                if found.len() > keep_back {
                    let cut = found.len() - keep_back;
                    let next = from + found[cut].range.start as u64;
                    found.truncate(cut);
                    next
                } else if (chunk as u64) < limit {
                    // Too few blocks: the stretch may cut one. Grow.
                    chunk = chunk.saturating_mul(2);
                    continue;
                } else {
                    return Ok((Vec::new(), from));
                }
            };
            let blocks = found
                .into_iter()
                .map(|f| Scanned {
                    hash: xxh3_64(&bytes[f.range.clone()]),
                    range: from + f.range.start as u64..from + f.range.end as u64,
                    kind: f.kind,
                    may_vanish: f.may_vanish,
                })
                .collect();
            return Ok((blocks, next));
        }
    }

    /// Indexes one more stretch after the frontier; returns the range of new
    /// block indices. Call until [`is_complete`](Self::is_complete).
    pub fn extend<T: Text + ?Sized>(&mut self, text: &T) -> io::Result<Range<usize>> {
        self.len = text.len();
        let first = self.blocks.len();
        if self.frontier >= self.len {
            return Ok(first..first);
        }
        let (found, next) = self.scan_stretch(text, self.frontier, self.chunk, u64::MAX)?;
        for s in found {
            let id = self.id();
            self.blocks.push(s.into_block(id));
        }
        self.frontier = next;
        Ok(first..self.blocks.len())
    }

    /// Extends the index until it covers `offset` (or the text ends).
    pub fn extend_to<T: Text + ?Sized>(&mut self, text: &T, offset: u64) -> io::Result<()> {
        self.len = text.len();
        while self.frontier <= offset && self.frontier < self.len {
            self.extend(text)?;
        }
        Ok(())
    }

    /// Index of the block containing `offset`, or of the first block after it.
    pub fn block_at(&self, offset: u64) -> usize {
        self.blocks.partition_point(|b| b.end <= offset)
    }

    /// Applies an edit made to the text: `removed` bytes at `from` were
    /// replaced by `inserted` bytes; `text` is the text after the edit.
    /// Re-parses at most about `budget` bytes past the edit before leaving the
    /// rest to [`extend`](Self::extend).
    pub fn edit<T: Text + ?Sized>(
        &mut self,
        text: &T,
        from: u64,
        removed: u64,
        inserted: u64,
        budget: u64,
    ) -> io::Result<Change> {
        let delta = inserted as i64 - removed as i64;
        let shift = |p: u64| (p as i64 + delta) as u64;
        let old_end = from + removed;
        let new_end = from + inserted;
        self.len = text.len();

        // An edit that adds the closing line to a leading `---` line makes
        // front matter of everything above it: start over (rare).
        let had_front_matter = self
            .blocks
            .first()
            .is_some_and(|b| b.kind == BlockKind::FrontMatter);
        if self.flavor.front_matter && from > 0 && !had_front_matter && from < FRONT_MATTER_MAX {
            let head = text.read(0..5)?;
            if (head.starts_with(b"---\n") || head.starts_with(b"---\r\n"))
                && front_matter_end_within(text, new_end + 4096)?.is_some()
            {
                let removed: Vec<u64> = self.blocks.drain(..).map(|b| b.id).collect();
                self.frontier = 0;
                return Ok(Change {
                    first: 0,
                    inserted: 0,
                    replaced: removed.clone(),
                    removed,
                    scanned: 0,
                    truncated: true,
                });
            }
        }

        // Edits beyond the frontier don't touch the indexed blocks once a
        // blank line lies between: whether a block starts at the frontier
        // (and so where the last indexed block ends) is settled by then. Its
        // first line decides the kind (`<div>` can interrupt a paragraph,
        // `<paradiv>` can't), and a paragraph made only of link reference
        // definitions has no node, which its following lines decide.
        if from > self.frontier {
            let probe = text.read(self.frontier..from)?;
            if has_blank_line(&probe) {
                return Ok(Change {
                    first: self.blocks.len(),
                    ..Change::default()
                });
            }
        }

        // Re-scan from the block before the one the edit touches (from the
        // start of the text if there is none). The touched block holds the
        // byte before the edit: text appended to a block's last line belongs
        // to it.
        let k = self.block_at(from.saturating_sub(1));
        let mut first = k.saturating_sub(1);
        // A paragraph that may become definitions only isn't a safe place to
        // start: go back past it.
        while first > 0 && self.blocks[first].may_vanish {
            first -= 1;
        }
        let start = if k == 0 { 0 } else { self.blocks[first].start };
        // Old blocks that start at or after the edit's end: the re-scan can
        // line up with any of them.
        let tail_from = self.blocks.partition_point(|b| b.start < old_end);
        let last_tail_start = self.blocks[tail_from..].last().map(|b| shift(b.start));

        let mut fresh: Vec<Scanned> = Vec::new();
        let mut pos = start;
        let mut scanned = 0u64;
        // (index of the first old block kept, new frontier, truncated)
        let (tail, frontier, truncated) = loop {
            // Edits re-scan in small stretches: most line up within a few
            // blocks. A stretch grows when one block doesn't fit.
            let chunk = EDIT_CHUNK.min(self.chunk);
            let limit = budget.saturating_sub(scanned).max(chunk as u64);
            let (found, next) = self.scan_stretch(text, pos, chunk, limit)?;
            if next == pos && next < self.len {
                // One block longer than the budget is still open here: the
                // index ends at its start until the background reaches its end.
                break (None, pos, true);
            }
            scanned += next.saturating_sub(pos);
            // Never line up at 0: only there does front matter start.
            let aligned = found.iter().enumerate().find_map(|(i, s)| {
                (s.range.start >= new_end && s.range.start > 0)
                    .then(|| self.find_tail(tail_from, shift, s.range.start))
                    .flatten()
                    .map(|j| (i, j))
            });
            if let Some((i, j)) = aligned {
                fresh.extend(found.into_iter().take(i));
                break (Some(j), shift(self.frontier), false);
            }
            fresh.extend(found);
            if next >= self.len {
                break (None, self.len, false);
            }
            if next >= new_end {
                if let Some(j) = self.find_tail(tail_from, shift, next) {
                    break (Some(j), shift(self.frontier), false);
                }
                // No old block left to line up with: the rest is unindexed.
                if last_tail_start.is_none_or(|s| s < next) {
                    break (None, next, false);
                }
                if scanned >= budget {
                    break (None, next, true);
                }
            }
            pos = next;
        };

        // Reuse the ids of replaced blocks with the same bytes, so unchanged
        // blocks aren't re-rendered.
        let replaced_end = tail.unwrap_or(self.blocks.len());
        let replaced = &self.blocks[first..replaced_end];
        let replaced_ids: Vec<u64> = replaced.iter().map(|b| b.id).collect();
        let mut pool: Vec<Option<&Block>> = replaced.iter().map(Some).collect();
        let mut next_id = self.next_id;
        let mut new_blocks = Vec::with_capacity(fresh.len());
        for s in fresh {
            let reuse = pool
                .iter_mut()
                .find(|b| b.is_some_and(|b| b.hash == s.hash && b.kind == s.kind))
                .and_then(Option::take);
            let id = match reuse {
                Some(b) => b.id,
                None => {
                    next_id += 1;
                    next_id - 1
                }
            };
            new_blocks.push(s.into_block(id));
        }
        let removed_ids: Vec<u64> = pool.into_iter().flatten().map(|b| b.id).collect();
        self.next_id = next_id;
        // The kept old blocks only move.
        if delta != 0 {
            for b in &mut self.blocks[replaced_end..] {
                b.start = shift(b.start);
                b.end = shift(b.end);
            }
        }
        let inserted_count = new_blocks.len();
        self.blocks.splice(first..replaced_end, new_blocks);
        self.frontier = frontier;
        Ok(Change {
            first,
            inserted: inserted_count,
            removed: removed_ids,
            replaced: replaced_ids,
            scanned,
            truncated,
        })
    }
    /// Index (in the current, unshifted blocks) of the old block at or after
    /// `tail_from` whose shifted start is `start`.
    fn find_tail(&self, tail_from: usize, shift: impl Fn(u64) -> u64, start: u64) -> Option<usize> {
        let tail = &self.blocks[tail_from..];
        let i = tail.partition_point(|b| shift(b.start) < start);
        // Front matter is front matter only at 0: moved, its lines are
        // ordinary blocks again (md_incremental fuzz).
        (i < tail.len() && shift(tail[i].start) == start && tail[i].kind != BlockKind::FrontMatter)
            .then_some(tail_from + i)
    }
}

#[cfg(test)]
mod tests;
