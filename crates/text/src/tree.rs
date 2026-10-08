//! The piece tree (§6.1; ADR-0008): a document is a sequence of pieces, each
//! a span of one buffer (the mapped original, the append-only add buffer, or
//! a derived buffer). Pieces live in a persistent treap: nodes are immutable
//! and shared through `Arc`, so an edit copies only the nodes on its path,
//! and a snapshot of the document (for search, save or undo) is one `Arc`
//! clone.
//!
//! Every node caches a [`Summary`] of its subtree: bytes, known line breaks,
//! and bytes whose breaks aren't known yet (original pieces in a region the
//! background index hasn't reached). Offset lookups are O(log n); line
//! lookups are O(log n) once the pieces before the target are indexed and
//! `None` before that, so callers can fall back to estimates.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub type BufferId = u32;

/// A span of a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub buf: BufferId,
    pub start: u64,
    pub len: u64,
    /// Line breaks in the span (see `index` for the counting rule), if known.
    pub breaks: Option<u64>,
}

impl Piece {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub bytes: u64,
    /// Breaks in pieces whose count is known.
    pub breaks: u64,
    /// Bytes in pieces whose count isn't known yet.
    pub unknown: u64,
}

impl Summary {
    fn of(p: &Piece) -> Self {
        match p.breaks {
            Some(b) => Self {
                bytes: p.len,
                breaks: b,
                unknown: 0,
            },
            None => Self {
                bytes: p.len,
                breaks: 0,
                unknown: p.len,
            },
        }
    }

    fn add(self, o: Self) -> Self {
        Self {
            bytes: self.bytes + o.bytes,
            breaks: self.breaks + o.breaks,
            unknown: self.unknown + o.unknown,
        }
    }

    pub fn is_exact(&self) -> bool {
        self.unknown == 0
    }
}

/// What the tree needs from the buffers: line-break counts of spans.
pub trait PieceContext {
    /// Breaks in `len` bytes of buffer `buf` from `start`, if known.
    fn breaks(&self, buf: BufferId, start: u64, len: u64) -> Option<u64>;

    /// Buffer offset just past the `k`-th (1-based) break in the span.
    /// `Some(None)`: the span has fewer than `k` breaks; `None`: unknown.
    fn nth_break(&self, buf: BufferId, start: u64, len: u64, k: u64) -> Option<Option<u64>>;
}

struct Node {
    piece: Piece,
    prio: u64,
    left: Link,
    right: Link,
    sum: Summary,
}

type Link = Option<Arc<Node>>;

fn sum(l: &Link) -> Summary {
    l.as_ref().map_or_else(Summary::default, |n| n.sum)
}

fn make(piece: Piece, prio: u64, left: Link, right: Link) -> Link {
    let s = sum(&left).add(Summary::of(&piece)).add(sum(&right));
    Some(Arc::new(Node {
        piece,
        prio,
        left,
        right,
        sum: s,
    }))
}

static PRIO: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Deterministic pseudo-random priorities (splitmix64 of a counter).
fn next_prio() -> u64 {
    let mut z = PRIO.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn merge(a: Link, b: Link) -> Link {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            if a.prio >= b.prio {
                let right = merge(a.right.clone(), Some(b));
                make(a.piece, a.prio, a.left.clone(), right)
            } else {
                let left = merge(Some(a), b.left.clone());
                make(b.piece, b.prio, left, b.right.clone())
            }
        }
    }
}

/// Splits so the left part holds the first `at` bytes. A piece straddling
/// `at` is cut in two, with break counts recomputed through `ctx`.
fn split(t: &Link, at: u64, ctx: &dyn PieceContext) -> (Link, Link) {
    let Some(n) = t else {
        return (None, None);
    };
    let l = sum(&n.left).bytes;
    let plen = n.piece.len;
    if at <= l {
        let (ll, lr) = split(&n.left, at, ctx);
        (ll, make(n.piece, n.prio, lr, n.right.clone()))
    } else if at >= l + plen {
        let (rl, rr) = split(&n.right, at - l - plen, ctx);
        (make(n.piece, n.prio, n.left.clone(), rl), rr)
    } else {
        let k = at - l;
        let p = n.piece;
        let head = Piece {
            buf: p.buf,
            start: p.start,
            len: k,
            breaks: ctx.breaks(p.buf, p.start, k),
        };
        let tail = Piece {
            buf: p.buf,
            start: p.start + k,
            len: p.len - k,
            breaks: ctx.breaks(p.buf, p.start + k, p.len - k),
        };
        (
            make(head, n.prio, n.left.clone(), None),
            make(tail, n.prio, None, n.right.clone()),
        )
    }
}

/// Builds a balanced treap over `pieces` in order: random priorities,
/// sorted so every parent outranks its children.
fn build(pieces: &[Piece]) -> Link {
    let mut prios: Vec<u64> = (0..pieces.len()).map(|_| next_prio()).collect();
    prios.sort_unstable_by(|a, b| b.cmp(a));
    // Breadth-first slot of each in-order position in a balanced tree.
    fn assign(lo: usize, hi: usize, slot: usize, out: &mut [usize]) {
        if lo >= hi {
            return;
        }
        let mid = lo + (hi - lo) / 2;
        out[mid] = slot;
        assign(lo, mid, 2 * slot + 1, out);
        assign(mid + 1, hi, 2 * slot + 2, out);
    }
    let mut slots = vec![0usize; pieces.len()];
    assign(0, pieces.len(), 0, &mut slots);
    // Rank slots: lower slot (closer to the root) gets a higher priority.
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_unstable_by_key(|&i| slots[i]);
    let mut prio_of = vec![0u64; pieces.len()];
    for (rank, &i) in order.iter().enumerate() {
        prio_of[i] = prios[rank];
    }
    fn go(pieces: &[Piece], prio_of: &[u64], lo: usize, hi: usize) -> Link {
        if lo >= hi {
            return None;
        }
        let mid = lo + (hi - lo) / 2;
        let left = go(pieces, prio_of, lo, mid);
        let right = go(pieces, prio_of, mid + 1, hi);
        make(pieces[mid], prio_of[mid], left, right)
    }
    go(pieces, &prio_of, 0, pieces.len())
}

/// A persistent sequence of pieces. Cloning is O(1).
#[derive(Clone, Default)]
pub struct PieceTree {
    root: Link,
}

impl std::fmt::Debug for PieceTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PieceTree")
            .field("summary", &self.summary())
            .finish()
    }
}

impl PieceTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tree over `pieces` in order. Empty pieces are dropped.
    pub fn from_pieces(pieces: &[Piece]) -> Self {
        let pieces: Vec<Piece> = pieces.iter().copied().filter(|p| p.len > 0).collect();
        Self {
            root: build(&pieces),
        }
    }

    pub fn summary(&self) -> Summary {
        sum(&self.root)
    }

    pub fn len(&self) -> u64 {
        self.summary().bytes
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Whether two trees are the same version (shared root).
    pub fn ptr_eq(&self, other: &PieceTree) -> bool {
        match (&self.root, &other.root) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Splits into the first `at` bytes and the rest.
    pub fn split_at(&self, at: u64, ctx: &dyn PieceContext) -> (PieceTree, PieceTree) {
        let (a, b) = split(&self.root, at.min(self.len()), ctx);
        (PieceTree { root: a }, PieceTree { root: b })
    }

    /// `self` followed by `other`.
    pub fn concat(&self, other: &PieceTree) -> PieceTree {
        PieceTree {
            root: merge(self.root.clone(), other.root.clone()),
        }
    }

    /// Inserts `other` at byte offset `at`.
    pub fn insert(&self, at: u64, other: &PieceTree, ctx: &dyn PieceContext) -> PieceTree {
        let (a, b) = self.split_at(at, ctx);
        a.concat(other).concat(&b)
    }

    /// Removes `range`; returns the new tree and the removed pieces.
    pub fn remove(&self, from: u64, to: u64, ctx: &dyn PieceContext) -> (PieceTree, PieceTree) {
        let (a, rest) = self.split_at(from, ctx);
        let (mid, b) = rest.split_at(to.saturating_sub(from), ctx);
        (a.concat(&b), mid)
    }

    /// If the piece that ends exactly at `at` ends where `extra` starts in
    /// the same buffer, grows it by `extra` (typing appends to the add
    /// buffer, so consecutive keystrokes stay one piece).
    pub fn extend_at(&self, at: u64, extra: &Piece, ctx: &dyn PieceContext) -> Option<PieceTree> {
        fn go(t: &Link, at: u64, extra: &Piece, ctx: &dyn PieceContext) -> Option<Link> {
            let n = t.as_ref()?;
            let l = sum(&n.left).bytes;
            let end = l + n.piece.len;
            if at <= l {
                let left = go(&n.left, at, extra, ctx)?;
                return Some(make(n.piece, n.prio, left, n.right.clone()));
            }
            if at > end {
                let right = go(&n.right, at - end, extra, ctx)?;
                return Some(make(n.piece, n.prio, n.left.clone(), right));
            }
            if at < end {
                return None;
            }
            let p = n.piece;
            if p.buf != extra.buf || p.end() != extra.start {
                return None;
            }
            // Recounted, not summed: a CR at the old end and an LF at the
            // start of `extra` become one CRLF.
            let grown = Piece {
                buf: p.buf,
                start: p.start,
                len: p.len + extra.len,
                breaks: ctx.breaks(p.buf, p.start, p.len + extra.len),
            };
            Some(make(grown, n.prio, n.left.clone(), n.right.clone()))
        }
        if at == 0 {
            return None;
        }
        go(&self.root, at, extra, ctx).map(|root| PieceTree { root })
    }

    /// The piece holding byte `offset` and the document offset it starts at.
    pub fn piece_at(&self, offset: u64) -> Option<(u64, Piece)> {
        let mut t = &self.root;
        let mut base = 0;
        let mut offset = offset;
        while let Some(n) = t {
            let l = sum(&n.left).bytes;
            if offset < l {
                t = &n.left;
            } else if offset < l + n.piece.len {
                return Some((base + l, n.piece));
            } else {
                offset -= l + n.piece.len;
                base += l + n.piece.len;
                t = &n.right;
            }
        }
        None
    }

    /// Calls `f(doc_offset, piece)` for every piece overlapping `from..to`,
    /// in order, until it breaks.
    pub fn visit(
        &self,
        from: u64,
        to: u64,
        f: &mut dyn FnMut(u64, &Piece) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        fn go(
            t: &Link,
            base: u64,
            from: u64,
            to: u64,
            f: &mut dyn FnMut(u64, &Piece) -> ControlFlow<()>,
        ) -> ControlFlow<()> {
            let Some(n) = t else {
                return ControlFlow::Continue(());
            };
            if base >= to || base + n.sum.bytes <= from {
                return ControlFlow::Continue(());
            }
            let l = sum(&n.left).bytes;
            go(&n.left, base, from, to, f)?;
            let ps = base + l;
            if ps < to && ps + n.piece.len > from {
                f(ps, &n.piece)?;
            }
            go(&n.right, ps + n.piece.len, from, to, f)
        }
        go(&self.root, 0, from, to, f)
    }

    /// All pieces in order.
    pub fn pieces(&self) -> Vec<Piece> {
        let mut out = Vec::new();
        let _ = self.visit(0, u64::MAX, &mut |_, p| {
            out.push(*p);
            ControlFlow::Continue(())
        });
        out
    }

    pub fn piece_count(&self) -> usize {
        fn count(t: &Link) -> usize {
            t.as_ref()
                .map_or(0, |n| 1 + count(&n.left) + count(&n.right))
        }
        count(&self.root)
    }

    /// Exact number of line breaks before `offset`, if every piece before it
    /// is indexed. (An offset between a CR and its LF counts the CR.)
    pub fn breaks_before(&self, offset: u64, ctx: &dyn PieceContext) -> Option<u64> {
        let mut t = &self.root;
        let mut offset = offset.min(self.len());
        let mut acc = 0u64;
        while let Some(n) = t {
            let ls = sum(&n.left);
            if offset <= ls.bytes {
                t = &n.left;
                continue;
            }
            if ls.unknown > 0 {
                return None;
            }
            acc += ls.breaks;
            let within = offset - ls.bytes;
            let p = n.piece;
            if within < p.len {
                return Some(acc + ctx.breaks(p.buf, p.start, within)?);
            }
            acc += p.breaks?;
            offset = within - p.len;
            t = &n.right;
        }
        Some(acc)
    }

    /// Exact start offset of line `line` (0-based), if every piece before it
    /// is indexed. `Some(None)`: the document has fewer lines.
    pub fn line_start(&self, line: u64, ctx: &dyn PieceContext) -> Option<Option<u64>> {
        enum Miss {
            /// The subtree is fully known and has this many breaks.
            Short(u64),
            Unknown,
        }
        fn go(t: &Link, k: u64, ctx: &dyn PieceContext) -> Result<u64, Miss> {
            let Some(n) = t else {
                return Err(Miss::Short(0));
            };
            let ls = sum(&n.left);
            let lb = if ls.unknown == 0 && k > ls.breaks {
                ls.breaks
            } else {
                match go(&n.left, k, ctx) {
                    Ok(off) => return Ok(off),
                    Err(Miss::Unknown) => return Err(Miss::Unknown),
                    Err(Miss::Short(lb)) => lb,
                }
            };
            let k = k - lb;
            let p = n.piece;
            let pb = match p.breaks {
                Some(b) if k > b => b,
                _ => match ctx.nth_break(p.buf, p.start, p.len, k) {
                    Some(Some(at)) => return Ok(ls.bytes + (at - p.start)),
                    _ => return Err(Miss::Unknown),
                },
            };
            match go(&n.right, k - pb, ctx) {
                Ok(off) => Ok(ls.bytes + p.len + off),
                Err(Miss::Short(rb)) => Err(Miss::Short(lb + pb + rb)),
                Err(Miss::Unknown) => Err(Miss::Unknown),
            }
        }
        if line == 0 {
            return Some(Some(0));
        }
        match go(&self.root, line, ctx) {
            Ok(off) => Some(Some(off)),
            Err(Miss::Short(_)) => Some(None),
            Err(Miss::Unknown) => None,
        }
    }

    /// Rebuilds pieces whose break counts were unknown, asking `ctx` again
    /// (after the background index advanced). Shares untouched subtrees.
    pub fn refresh_unknown(&self, ctx: &dyn PieceContext) -> PieceTree {
        fn go(t: &Link, ctx: &dyn PieceContext) -> Link {
            let n = t.as_ref()?;
            if n.sum.unknown == 0 {
                return t.clone();
            }
            let mut p = n.piece;
            if p.breaks.is_none() {
                p.breaks = ctx.breaks(p.buf, p.start, p.len);
            }
            make(p, n.prio, go(&n.left, ctx), go(&n.right, ctx))
        }
        PieceTree {
            root: go(&self.root, ctx),
        }
    }

    /// Height of the tree, for tests and diagnostics.
    pub fn height(&self) -> usize {
        fn h(t: &Link) -> usize {
            t.as_ref().map_or(0, |n| 1 + h(&n.left).max(h(&n.right)))
        }
        h(&self.root)
    }
}

#[cfg(test)]
mod tests;
