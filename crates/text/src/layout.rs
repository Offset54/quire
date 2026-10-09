//! Visual rows (§6.1 long lines): with wrapping off a row is a line; with
//! wrapping on, lines are cut into rows of at most `wrap` columns, breaking
//! after whitespace or around wide (CJK) characters when possible.
//!
//! Wrapping is computed lazily, per visible line. A line longer than
//! [`SEG`] bytes is wrapped in independent segments of `SEG` bytes (each
//! starting a new row), so the rows around any offset of a 300 MB line cost
//! at most one segment of work. Columns past the first segment of a line
//! are approximated by byte distance (exact for ASCII).

use std::io;

use serde::{Deserialize, Serialize};

use crate::buffer::Snapshot;
use crate::chars::{chars, is_wide, width};

/// Segment length for long lines.
pub const SEG: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    /// Wrap column, or `None` for no wrapping.
    pub wrap: Option<u32>,
    pub tab: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { wrap: None, tab: 4 }
    }
}

/// One visual row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualRow {
    pub start: u64,
    /// End of the row's text (the line's break bytes are never inside).
    pub end: u64,
    pub line_start: u64,
    /// End of the line's text, before its break.
    pub line_end: u64,
    /// Start of the next line (= `line_end` on the last line).
    pub next_line: u64,
}

impl VisualRow {
    pub fn is_first(&self) -> bool {
        self.start == self.line_start
    }

    pub fn is_last(&self) -> bool {
        self.end == self.line_end
    }
}

fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// Moves a raw segment boundary forward past UTF-8 continuation bytes.
fn adjust(snap: &Snapshot, at: u64, limit: u64) -> io::Result<u64> {
    let mut at = at;
    for _ in 0..3 {
        if at >= limit || !is_continuation(snap.byte_at(at)?) {
            break;
        }
        at += 1;
    }
    Ok(at.min(limit))
}

/// Start of the wrap segment of the line at `line_start` holding `offset`.
pub fn segment_start(
    snap: &Snapshot,
    line_start: u64,
    line_end: u64,
    offset: u64,
) -> io::Result<u64> {
    let mut k = (offset.saturating_sub(line_start)) / SEG;
    loop {
        let s = adjust(snap, line_start + k * SEG, line_end)?;
        if s <= offset || k == 0 {
            return Ok(if k == 0 { line_start } else { s });
        }
        k -= 1;
    }
}

fn segment_end(snap: &Snapshot, line_start: u64, line_end: u64, seg: u64) -> io::Result<u64> {
    let k = (seg - line_start) / SEG + 1;
    let raw = line_start.saturating_add(k * SEG);
    if raw >= line_end {
        return Ok(line_end);
    }
    adjust(snap, raw, line_end)
}

/// Row starts of `bytes` wrapped at `wrap` columns (relative offsets; the
/// first is 0). Tabs are measured from the row start, as the view renders
/// each row on its own.
pub fn wrap_rows(bytes: &[u8], wrap: u32, tab: u32) -> Vec<usize> {
    let wrap = wrap.max(1);
    let mut starts = vec![0usize];
    let mut row_start = 0usize;
    let mut col = 0u32;
    // Where the next row may start, and the columns used before it.
    let mut opp: Option<(usize, u32)> = None;
    for (off, n, c) in chars(bytes) {
        let wide = c.is_some_and(is_wide);
        if wide && off > row_start {
            opp = Some((off, col));
        }
        let mut w = width(c, col, tab);
        if col + w > wrap && col > 0 && off > row_start {
            let (at, used) = match opp {
                Some((p, pc)) if p > row_start && p <= off => (p, col - pc),
                _ => (off, 0),
            };
            starts.push(at);
            row_start = at;
            col = used;
            opp = None;
            w = width(c, col, tab);
        }
        col += w;
        if wide || matches!(c, Some(' ' | '\t')) {
            opp = Some((off + n, col));
        }
    }
    starts
}

/// The line holding `offset`: (start, end of text, next line start).
pub fn line_bounds(snap: &Snapshot, offset: u64) -> io::Result<(u64, u64, u64)> {
    let start = snap.line_start_containing(offset)?;
    let (end, next) = snap.line_end(start)?;
    Ok((start, end, next))
}

/// Rows of the segment starting at `seg` of the given line.
fn segment_rows(
    snap: &Snapshot,
    (line_start, line_end, next_line): (u64, u64, u64),
    seg: u64,
    layout: &Layout,
) -> io::Result<Vec<VisualRow>> {
    let row = |start, end| VisualRow {
        start,
        end,
        line_start,
        line_end,
        next_line,
    };
    let Some(wrap) = layout.wrap else {
        return Ok(vec![row(line_start, line_end)]);
    };
    let end = segment_end(snap, line_start, line_end, seg)?;
    let bytes = snap.read(seg..end)?;
    let starts = wrap_rows(&bytes, wrap, layout.tab);
    Ok(starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).map_or(end, |&e| seg + e as u64);
            row(seg + s as u64, e)
        })
        .collect())
}

/// The row holding `offset`. An offset at the boundary between two rows of
/// a line belongs to the later one.
pub fn row_containing(snap: &Snapshot, offset: u64, layout: &Layout) -> io::Result<VisualRow> {
    let bounds = line_bounds(snap, offset)?;
    let off = offset.min(bounds.1);
    let seg = if layout.wrap.is_some() {
        segment_start(snap, bounds.0, bounds.1, off)?
    } else {
        bounds.0
    };
    let rows = segment_rows(snap, bounds, seg, layout)?;
    Ok(*rows
        .iter()
        .rev()
        .find(|r| r.start <= off)
        .unwrap_or(&rows[0]))
}

pub fn row_after(
    snap: &Snapshot,
    row: &VisualRow,
    layout: &Layout,
) -> io::Result<Option<VisualRow>> {
    if row.end < row.line_end {
        return row_containing(snap, row.end, layout).map(Some);
    }
    if row.next_line > row.line_end {
        return row_containing(snap, row.next_line, layout).map(Some);
    }
    Ok(None)
}

pub fn row_before(
    snap: &Snapshot,
    row: &VisualRow,
    layout: &Layout,
) -> io::Result<Option<VisualRow>> {
    if row.start > row.line_start {
        return row_containing(snap, row.start - 1, layout).map(Some);
    }
    if row.line_start == 0 {
        return Ok(None);
    }
    // The previous line's break: step onto its first byte (the CR of a
    // CRLF), never between CR and LF.
    let mut p = row.line_start - 1;
    if p >= 1 && snap.read(p - 1..p + 1)? == b"\r\n" {
        p -= 1;
    }
    row_containing(snap, p, layout).map(Some)
}

/// Up to `before` rows above the row holding `anchor`, that row, and up to
/// `after` rows below it. Returns the rows and the anchor row's index.
///
/// Works a segment (or an unwrapped line) at a time, so each segment is
/// read and wrapped once however many of its rows are wanted.
pub fn rows_around(
    snap: &Snapshot,
    anchor: u64,
    before: usize,
    after: usize,
    layout: &Layout,
) -> io::Result<(Vec<VisualRow>, usize)> {
    let bounds = line_bounds(snap, anchor.min(snap.len()))?;
    let off = anchor.min(bounds.1);
    let seg = if layout.wrap.is_some() {
        segment_start(snap, bounds.0, bounds.1, off)?
    } else {
        bounds.0
    };
    let rows = segment_rows(snap, bounds, seg, layout)?;
    let i = rows.iter().rposition(|r| r.start <= off).unwrap_or(0);
    // `up` is in reverse order (nearest first).
    let mut up: Vec<VisualRow> = rows[..i].iter().rev().copied().collect();
    let mut down: Vec<VisualRow> = rows[i..].to_vec();
    let mut first = rows[0];
    while up.len() < before {
        let Some(chunk) = segment_before(snap, &first, layout)? else {
            break;
        };
        first = chunk[0];
        up.extend(chunk.iter().rev());
    }
    let mut last = *rows.last().expect("a segment has a row");
    while down.len() < after + 1 {
        let Some(chunk) = segment_after(snap, &last, layout)? else {
            break;
        };
        last = *chunk.last().expect("a segment has a row");
        down.extend(chunk);
    }
    up.truncate(before);
    down.truncate(after + 1);
    let index = up.len();
    up.reverse();
    up.extend(down);
    Ok((up, index))
}

/// Rows of the segment (or line) after the one ending with `last`.
fn segment_after(
    snap: &Snapshot,
    last: &VisualRow,
    layout: &Layout,
) -> io::Result<Option<Vec<VisualRow>>> {
    if last.end < last.line_end {
        let bounds = (last.line_start, last.line_end, last.next_line);
        return segment_rows(snap, bounds, last.end, layout).map(Some);
    }
    if last.next_line > last.line_end {
        let start = last.next_line;
        let (end, next) = snap.line_end(start)?;
        return segment_rows(snap, (start, end, next), start, layout).map(Some);
    }
    Ok(None)
}

/// Rows of the segment (or line) before the one starting with `first`.
fn segment_before(
    snap: &Snapshot,
    first: &VisualRow,
    layout: &Layout,
) -> io::Result<Option<Vec<VisualRow>>> {
    let (bounds, at) = if first.start > first.line_start {
        let b = (first.line_start, first.line_end, first.next_line);
        (b, first.start - 1)
    } else if first.line_start == 0 {
        return Ok(None);
    } else {
        // The previous line's break: step onto its first byte (the CR of a
        // CRLF), never between CR and LF. Then its last segment.
        let mut p = first.line_start - 1;
        if p >= 1 && snap.read(p - 1..p + 1)? == b"\r\n" {
            p -= 1;
        }
        let b = line_bounds(snap, p)?;
        (b, b.1.saturating_sub(1).max(b.0))
    };
    let seg = if layout.wrap.is_some() {
        segment_start(snap, bounds.0, bounds.1, at)?
    } else {
        bounds.0
    };
    segment_rows(snap, bounds, seg, layout).map(Some)
}

/// Visual column of `offset` within `row` (row-relative; approximate past
/// the first segment of a long unwrapped line).
pub fn column_in_row(snap: &Snapshot, row: &VisualRow, offset: u64, tab: u32) -> io::Result<u32> {
    let offset = offset.clamp(row.start, row.end);
    let base = if offset - row.start > SEG {
        let s = segment_start(snap, row.start, row.end, offset)?;
        Some(s)
    } else {
        None
    };
    let from = base.unwrap_or(row.start);
    let mut col = (from - row.start) as u32;
    for (_, _, c) in chars(&snap.read(from..offset)?) {
        col += width(c, col, tab);
    }
    Ok(col)
}

/// Offset in `row` closest to visual column `col` (never inside a
/// character; a character straddling `col` snaps to whichever edge is
/// nearer).
pub fn offset_at_column(snap: &Snapshot, row: &VisualRow, col: u32, tab: u32) -> io::Result<u64> {
    let (from, mut at_col) = if u64::from(col) > SEG && row.end - row.start > SEG {
        let target = (row.start + u64::from(col)).min(row.end);
        let s = segment_start(snap, row.start, row.end, target)?;
        (s, (s - row.start) as u32)
    } else {
        (row.start, 0)
    };
    let end = row.end.min(from + SEG * 2);
    let bytes = snap.read(from..end)?;
    for (off, _, c) in chars(&bytes) {
        let w = width(c, at_col, tab);
        if at_col + w > col {
            let nearer_right = col - at_col > w / 2 && w > 1;
            let n = crate::chars::next_grapheme(&bytes, off);
            return Ok(from + if nearer_right { n } else { off } as u64);
        }
        at_col += w;
    }
    Ok(end)
}

#[cfg(test)]
mod tests;
