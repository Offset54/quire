//! Replace-all as one transaction (§3.1, §6.1): the result is computed off
//! the document lock from a snapshot, then swapped in as a single undoable
//! step.
//!
//! - Up to [`PIECE_LIMIT`] matches, the new document is a list of pieces:
//!   the untouched spans of the old pieces, and the replacement texts in the
//!   add buffer. Nothing is copied, and untouched bytes keep their origin
//!   (so they still save byte-for-byte).
//! - Past that, piece overhead would dominate, so the result is streamed
//!   into a new buffer (in memory up to 64 MiB, else a delete-on-close temp
//!   file that is then mapped) and indexed.

use std::io::{self, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

use quire_core::scheduler::CancelToken;

use crate::buffer::{Adder, BufferEntry, Snapshot};
use crate::index::ChunkIndex;
use crate::search::{Matcher, for_each_replacement};
use crate::source::{MappedSource, MemorySource, Source};
use crate::tree::{Piece, PieceContext, PieceTree};

#[cfg(not(test))]
pub const PIECE_LIMIT: u64 = 500_000;
#[cfg(test)]
pub const PIECE_LIMIT: u64 = 50;
const IN_MEMORY_MAX: u64 = 64 << 20;

#[derive(Debug)]
pub struct ReplaceAll {
    pub tree: PieceTree,
    pub count: u64,
    /// Whether the result was written to a new buffer.
    pub materialized: bool,
}

/// Pieces of `snap` covering `from..to`.
fn clip(snap: &Snapshot, from: u64, to: u64, out: &mut Vec<Piece>) {
    let _ = snap.tree.visit(from, to, &mut |ps, p| {
        let a = from.max(ps);
        let b = to.min(ps + p.len);
        let piece = if a == ps && b == ps + p.len {
            *p
        } else {
            let start = p.start + (a - ps);
            Piece {
                buf: p.buf,
                start,
                len: b - a,
                breaks: snap.buffers.breaks(p.buf, start, b - a),
            }
        };
        out.push(piece);
        ControlFlow::Continue(())
    });
}

/// Computes the result of replacing every match. `None` if cancelled.
/// `progress(bytes_done)` reports how far the scan got.
pub fn compute(
    snap: &Snapshot,
    adder: &Adder,
    m: &Matcher,
    replacement: &str,
    temp_dir: &Path,
    cancel: &CancelToken,
    progress: &dyn Fn(u64),
) -> io::Result<Option<ReplaceAll>> {
    // Pass 1: pieces, with replacement texts gathered locally first.
    let mut spans: Vec<(u64, u64, u64, u64)> = Vec::new(); // (from, to, repl_start, repl_len)
    let mut repl = Vec::new();
    let mut too_many = false;
    let done = for_each_replacement(snap, m, replacement, cancel, &mut |(a, b), text| {
        if spans.len() as u64 >= PIECE_LIMIT {
            too_many = true;
            return Ok(ControlFlow::Break(()));
        }
        spans.push((a, b, repl.len() as u64, text.len() as u64));
        repl.extend_from_slice(text);
        if spans.len().is_multiple_of(4096) {
            progress(b);
        }
        Ok(ControlFlow::Continue(()))
    })?;
    if !done && !too_many {
        return Ok(None);
    }
    if too_many {
        return materialize(snap, adder, m, replacement, temp_dir, cancel, progress);
    }
    let count = spans.len() as u64;
    let base = if repl.is_empty() {
        None
    } else {
        Some(adder.append(&repl)?)
    };
    let mut pieces = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0;
    for &(a, b, rs, rl) in &spans {
        clip(snap, at, a, &mut pieces);
        if rl > 0 {
            let base = base.expect("replacements were appended");
            let start = base.start + rs;
            pieces.push(Piece {
                buf: base.buf,
                start,
                len: rl,
                breaks: adder.buffers.breaks(base.buf, start, rl),
            });
        }
        at = b;
    }
    clip(snap, at, snap.len(), &mut pieces);
    fix_crlf_junctions(&mut pieces, adder)?;
    Ok(Some(ReplaceAll {
        tree: PieceTree::from_pieces(&pieces),
        count,
        materialized: false,
    }))
}

/// Where one piece ends with CR and the next starts with LF, both are cut
/// by one byte and a CRLF piece goes between them, so the pair counts as
/// one line break.
fn fix_crlf_junctions(pieces: &mut Vec<Piece>, adder: &Adder) -> io::Result<()> {
    let byte = |p: &Piece, at: u64| -> io::Result<u8> {
        let mut b = [0u8];
        adder.buffers.get(p.buf).source.read_at(at, &mut b)?;
        Ok(b[0])
    };
    let mut i = 1;
    let mut crlf: Option<Piece> = None;
    while i < pieces.len() {
        let (prev, next) = (pieces[i - 1], pieces[i]);
        if prev.len > 0
            && next.len > 0
            && byte(&prev, prev.end() - 1)? == b'\r'
            && byte(&next, next.start)? == b'\n'
        {
            let joint = match crlf {
                Some(p) => p,
                None => {
                    let p = adder.append(b"\r\n")?;
                    crlf = Some(p);
                    p
                }
            };
            let ctx = &adder.buffers;
            let head = Piece {
                len: prev.len - 1,
                breaks: ctx.breaks(prev.buf, prev.start, prev.len - 1),
                ..prev
            };
            let tail = Piece {
                start: next.start + 1,
                len: next.len - 1,
                breaks: ctx.breaks(next.buf, next.start + 1, next.len - 1),
                ..next
            };
            pieces.splice(i - 1..=i, [head, joint, tail]);
            i += 2;
        } else {
            i += 1;
        }
    }
    pieces.retain(|p| p.len > 0);
    Ok(())
}

fn materialize(
    snap: &Snapshot,
    adder: &Adder,
    m: &Matcher,
    replacement: &str,
    temp_dir: &Path,
    cancel: &CancelToken,
    progress: &dyn Fn(u64),
) -> io::Result<Option<ReplaceAll>> {
    enum Out {
        Memory(Vec<u8>),
        File(io::BufWriter<std::fs::File>),
    }
    let mut out = if snap.len() <= IN_MEMORY_MAX {
        Out::Memory(Vec::with_capacity(snap.len() as usize))
    } else {
        std::fs::create_dir_all(temp_dir)?;
        Out::File(io::BufWriter::with_capacity(
            8 << 20,
            tempfile::tempfile_in(temp_dir)?,
        ))
    };
    let mut write = |bytes: &[u8]| -> io::Result<()> {
        match &mut out {
            Out::Memory(v) => v.extend_from_slice(bytes),
            Out::File(w) => w.write_all(bytes)?,
        }
        Ok(())
    };
    let mut at = 0u64;
    let mut count = 0u64;
    let copy = |from: u64, to: u64, write: &mut dyn FnMut(&[u8]) -> io::Result<()>| {
        snap.for_each_block(from..to, 4 << 20, &mut |_, b| {
            write(b)?;
            Ok(ControlFlow::Continue(()))
        })
    };
    let done = for_each_replacement(snap, m, replacement, cancel, &mut |(a, b), text| {
        copy(at, a, &mut write)?;
        write(text)?;
        at = b;
        count += 1;
        if count.is_multiple_of(65536) {
            progress(b);
        }
        Ok(ControlFlow::Continue(()))
    })?;
    if !done {
        return Ok(None);
    }
    copy(at, snap.len(), &mut write)?;
    let source: Arc<dyn Source> = match out {
        Out::Memory(v) => Arc::new(MemorySource::new(v)),
        Out::File(w) => {
            let f = w.into_inner().map_err(io::IntoInnerError::into_error)?;
            if f.metadata()?.len() == 0 {
                Arc::new(MemorySource::new(Vec::new()))
            } else {
                Arc::new(MappedSource::new(f)?)
            }
        }
    };
    let index = Arc::new(ChunkIndex::unindexed(source.len()));
    source.prefetch(0, source.len());
    index.scan_range(&*source, 0, index.chunk_count(), None)?;
    let len = source.len();
    let breaks = index.piece_breaks(&*source, 0, len)?;
    let id = adder.buffers.push(BufferEntry { source, index });
    Ok(Some(ReplaceAll {
        tree: PieceTree::from_pieces(&[Piece {
            buf: id,
            start: 0,
            len,
            breaks,
        }]),
        count,
        materialized: true,
    }))
}

#[cfg(test)]
mod tests;
