//! What the TXT view needs for a viewport, as one binary message (§5.6:
//! text ranges never travel as JSON).
//!
//! Layout: `[u32 header length][header JSON][rows]`. The small header holds
//! revision, counts, selections and highlights in the window; each row is
//!
//! ```text
//! u64 start, u64 end, u64 text_start, u64 line (u64::MAX: not shown),
//! u32 flags (1 first row of its line, 2 last row, 4 a line break follows),
//! u32 col0 (visual column of text_start), u32 n_subst, u32 subst[n],
//! u32 text_len, text (UTF-8)
//! ```
//!
//! Rows longer than [`MAX_ROW_TEXT`] bytes (unwrapped long lines) carry only
//! the text around the requested columns.

use std::io;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::buffer::LineNumber;
use crate::chars::display_text;
use crate::document::TextDocument;
use crate::layout::{Layout, column_in_row, offset_at_column, rows_around};

pub const MAX_ROW_TEXT: u64 = 8 * 1024;
/// Columns sent either side of the requested window in long rows.
const HMARGIN: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewRequest {
    /// Offset whose row anchors the window.
    pub anchor: u64,
    /// Anchor at the primary caret instead (after an edit, wherever it went).
    #[serde(default)]
    pub caret: bool,
    pub before: u32,
    pub after: u32,
    pub layout: Layout,
    /// Visible column range, for long unwrapped rows.
    pub hcols: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewHeader {
    pub revision: u64,
    pub len: u64,
    pub anchor_index: u32,
    pub rows: u32,
    pub line_count: LineNumber,
    /// Position of the anchor row in the document, 0..1, for the scrollbar:
    /// by line once lines are known exactly, by byte before that.
    pub fraction: f64,
    pub selections: Vec<(u64, u64)>,
    pub primary: (u64, u64),
    pub selection_count: usize,
    pub highlights: Vec<(u64, u64)>,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub read_only: bool,
    /// Time spent in the command up to this header, in microseconds (for
    /// splitting a round trip into core time and IPC).
    pub core_us: u64,
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Builds the view message. `highlights(from, to)` returns search matches
/// in that range.
pub fn build(
    doc: &TextDocument,
    req: &ViewRequest,
    highlights: &dyn Fn(u64, u64) -> Vec<(u64, u64)>,
) -> io::Result<Vec<u8>> {
    build_since(doc, req, highlights, Instant::now())
}

/// [`build`], reporting the time since `started` (the command's start) as
/// `core_us`.
pub fn build_since(
    doc: &TextDocument,
    req: &ViewRequest,
    highlights: &dyn Fn(u64, u64) -> Vec<(u64, u64)>,
    started: Instant,
) -> io::Result<Vec<u8>> {
    let snap = doc.snapshot();
    let anchor = if req.caret {
        doc.primary().head
    } else {
        req.anchor
    };
    let (rows, anchor_index) = rows_around(
        &snap,
        anchor,
        req.before as usize,
        req.after as usize,
        &req.layout,
    )?;
    let from = rows.first().map_or(0, |r| r.start);
    let to = rows.last().map_or(0, |r| r.next_line.max(r.end));
    let mut body = Vec::with_capacity(rows.len() * 128);
    let mut next_line: Option<u64> = None;
    for r in &rows {
        let (text_start, text_end) = if r.end - r.start > MAX_ROW_TEXT {
            let (c0, c1) = req.hcols.unwrap_or((0, 0));
            let a = offset_at_column(&snap, r, c0.saturating_sub(HMARGIN), req.layout.tab)?;
            let b = offset_at_column(&snap, r, c1 + HMARGIN, req.layout.tab)?;
            (a, b.max(a).min(a + 4 * MAX_ROW_TEXT))
        } else {
            (r.start, r.end)
        };
        let col0 = if text_start == r.start {
            0
        } else {
            column_in_row(&snap, r, text_start, req.layout.tab)?
        };
        let bytes = snap.read(text_start..text_end)?;
        let (text, subst) = display_text(&bytes);
        // Rows are consecutive: number the first line through the index,
        // then count.
        let line = if r.is_first() {
            let l = next_line.or_else(|| snap.line_of(r.start));
            next_line = l.map(|l| l + 1);
            l.unwrap_or(u64::MAX)
        } else {
            u64::MAX
        };
        let flags = u32::from(r.is_first())
            | (u32::from(r.is_last()) << 1)
            | (u32::from(r.next_line > r.line_end && r.is_last()) << 2);
        put64(&mut body, r.start);
        put64(&mut body, r.end);
        put64(&mut body, text_start);
        put64(&mut body, line);
        put32(&mut body, flags);
        put32(&mut body, col0);
        put32(&mut body, subst.len() as u32);
        for s in &subst {
            put32(&mut body, *s);
        }
        put32(&mut body, text.len() as u32);
        body.extend_from_slice(text.as_bytes());
    }
    let mut selections = Vec::new();
    doc.selections_in(from, to, &mut |s, _| {
        selections.push((s.anchor, s.head));
        std::ops::ControlFlow::Continue(())
    });
    let count = snap.line_count_estimate();
    let anchor_start = rows.get(anchor_index).map_or(0, |r| r.start);
    let fraction = match snap.line_count() {
        Some(n) if n > 1 => snap.line_of(anchor_start).unwrap_or(0) as f64 / (n - 1) as f64,
        _ if !snap.is_empty() => anchor_start as f64 / snap.len() as f64,
        _ => 0.0,
    };
    let p = doc.primary();
    let header = ViewHeader {
        revision: doc.revision(),
        len: snap.len(),
        anchor_index: anchor_index as u32,
        rows: rows.len() as u32,
        line_count: count,
        fraction,
        selections,
        primary: (p.anchor, p.head),
        selection_count: doc.selections().len(),
        highlights: highlights(from, to),
        dirty: doc.is_dirty(),
        can_undo: doc.can_undo(),
        can_redo: doc.can_redo(),
        read_only: doc.read_only,
        core_us: started.elapsed().as_micros() as u64,
    };
    let json = serde_json::to_vec(&header).map_err(io::Error::other)?;
    let mut out = Vec::with_capacity(4 + json.len() + body.len());
    put32(&mut out, json.len() as u32);
    out.extend_from_slice(&json);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Where the scrollbar fraction `f` (0..1) lands: a line start, by line
/// once lines are known exactly, else by byte offset (§6.1).
pub fn seek(doc: &TextDocument, f: f64) -> io::Result<u64> {
    let snap = doc.snapshot();
    let f = f.clamp(0.0, 1.0);
    match snap.line_count() {
        Some(n) => {
            let line = ((n - 1) as f64 * f).round() as u64;
            Ok(snap.line_start(line).flatten().unwrap_or(0))
        }
        None => snap.line_start_containing((snap.len() as f64 * f) as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Action;

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn view_message_layout() {
        let mut doc = TextDocument::from_bytes("one\ntw\u{1}o\nthree".as_bytes());
        doc.apply(&Action::SetSelections {
            ranges: vec![(5, 6)],
        })
        .unwrap();
        let req = ViewRequest {
            anchor: 5,
            caret: false,
            before: 5,
            after: 5,
            layout: Layout::default(),
            hcols: None,
        };
        let msg = build(&doc, &req, &|_, _| vec![(0, 3)]).unwrap();
        let hl = u32_at(&msg, 0) as usize;
        let h: serde_json::Value = serde_json::from_slice(&msg[4..4 + hl]).unwrap();
        assert_eq!(h["rows"], 3);
        assert_eq!(h["anchorIndex"], 1);
        assert_eq!(h["selections"][0], serde_json::json!([5, 6]));
        assert_eq!(h["highlights"][0], serde_json::json!([0, 3]));
        assert_eq!(h["lineCount"]["line"], 3);
        // Second row: "tw\u{1}o" with the control shown as a symbol.
        let mut at = 4 + hl;
        let row = |at: &mut usize| {
            let start = u64::from_le_bytes(msg[*at..*at + 8].try_into().unwrap());
            let line = u64::from_le_bytes(msg[*at + 24..*at + 32].try_into().unwrap());
            let flags = u32_at(&msg, *at + 32);
            let n = u32_at(&msg, *at + 40) as usize;
            let subst: Vec<u32> = (0..n).map(|i| u32_at(&msg, *at + 44 + 4 * i)).collect();
            let tl_at = *at + 44 + 4 * n;
            let tl = u32_at(&msg, tl_at) as usize;
            let text = String::from_utf8(msg[tl_at + 4..tl_at + 4 + tl].to_vec()).unwrap();
            *at = tl_at + 4 + tl;
            (start, line, flags, subst, text)
        };
        assert_eq!(row(&mut at), (0, 0, 7, vec![], "one".into()));
        assert_eq!(row(&mut at), (4, 1, 7, vec![2], "tw\u{2401}o".into()));
        assert_eq!(row(&mut at), (9, 2, 3, vec![], "three".into()));
        assert_eq!(at, msg.len());
        assert_eq!(seek(&doc, 1.0).unwrap(), 9);
        assert_eq!(seek(&doc, 0.5).unwrap(), 4);
    }
}
