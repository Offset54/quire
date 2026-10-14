//! Display pieces of large blocks (ADR-0030).
//!
//! The preview keeps, renders and patches whole blocks. A block with
//! thousands of rows or lines would cost tens of milliseconds to render and
//! to lay out in the webview on every keystroke inside it (md-20m-edit: 77–
//! 115 ms for a 1,500-row table), so large tables and fenced code blocks are
//! shown as pieces: each piece renders the block's head (a table's header
//! and delimiter rows, a code block's opening fence), a run of its rows or
//! lines, and its tail (a closing fence), and is an entry of its own in
//! preview windows, with its own source range. An edit inside the block
//! changes only the pieces whose bytes changed: piece boundaries are chosen
//! by the rows' content (a cut after a row whose hash hits a residue, within
//! a minimum and maximum run), so inserting or deleting a row moves no other
//! piece's boundary.
//!
//! Table pieces are separate `<table>` elements; they line up because all
//! of them get the same column widths (a `<colgroup>` from the widest cell of
//! each column over the whole table, `table-layout: fixed`). Only the first
//! piece shows the header.

use std::ops::Range;

use xxhash_rust::xxh3::{Xxh3, xxh3_64};

use crate::scan::BlockKind;
use crate::text::line_starts;

/// Piece ids have this bit set (block ids are counters from 1 and never
/// reach it; ids stay below 2^53, exact as JS numbers).
pub const PIECE_ID_BIT: u64 = 1 << 52;
/// Tables with more body rows than this are shown as pieces.
pub const TABLE_MIN_ROWS: usize = 100;
/// Fenced code blocks with more lines than this are shown as pieces.
pub const CODE_MIN_LINES: usize = 200;
/// Rows (lines) per piece: a cut after a row whose hash is 0 mod `AVG`,
/// at least `MIN` and at most `MAX` rows.
const MIN_RUN: usize = 16;
const AVG_RUN: u64 = 32;
const MAX_RUN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub id: u64,
    /// Byte range within the block (the block moves when text before it
    /// changes; its layout is cached by block id).
    pub start: u64,
    pub end: u64,
    /// The piece's own lines within the block's bytes.
    pub body: Range<usize>,
    pub lines: u32,
}

#[derive(Clone, Debug)]
pub struct Layout {
    table: bool,
    /// Rendered before each piece's body: a table's header and delimiter
    /// rows, a code block's opening fence.
    head: Vec<u8>,
    /// Rendered after each body: a code block's closing fence.
    tail: Vec<u8>,
    /// Table column widths, percent.
    widths: Vec<f32>,
    pub pieces: Vec<Piece>,
}

impl Layout {
    /// Source to render for a piece whose own lines are `body`.
    pub fn source(&self, body: &[u8]) -> Vec<u8> {
        let mut s = Vec::with_capacity(self.head.len() + body.len() + self.tail.len() + 1);
        s.extend_from_slice(&self.head);
        s.extend_from_slice(body);
        if !self.tail.is_empty() {
            if !s.ends_with(b"\n") {
                s.push(b'\n');
            }
            s.extend_from_slice(&self.tail);
        }
        s
    }

    /// Wraps piece `k`'s rendering: position classes and, for a table, the
    /// shared column widths.
    pub fn wrap(&self, k: usize, html: &str) -> String {
        let mut cls = String::from("md-piece");
        if k == 0 {
            cls.push_str(" md-piece-first");
        }
        if k + 1 == self.pieces.len() {
            cls.push_str(" md-piece-last");
        }
        let mut out = format!("<div class=\"{cls}\">");
        // `<table>` or `<table data-sourcepos="…">`.
        let tag = html
            .match_indices("<table")
            .find(|(at, _)| matches!(html.as_bytes().get(at + 6), Some(b'>' | b' ')))
            .and_then(|(at, _)| Some((at, at + html[at..].find('>')?)))
            .filter(|_| self.table);
        match tag {
            Some((at, close)) => {
                out.push_str(&html[..at]);
                out.push_str("<table class=\"md-piece-table\"");
                out.push_str(&html[at + "<table".len()..=close]);
                out.push_str("<colgroup>");
                for w in &self.widths {
                    // An attribute, not a style: the CSP has no inline styles.
                    out.push_str(&format!("<col width=\"{w:.2}%\">"));
                }
                out.push_str("</colgroup>");
                out.push_str(&html[close + 1..]);
            }
            None => out.push_str(html),
        }
        out.push_str("</div>");
        out
    }

    /// The piece holding byte `offset` of the block (the last one past the
    /// end).
    pub fn piece_at(&self, offset: u64) -> usize {
        self.pieces
            .iter()
            .position(|p| offset < p.end)
            .unwrap_or(self.pieces.len().saturating_sub(1))
    }
}

/// Pieces for a block of `kind` with `bytes`, or None if it is
/// shown whole.
pub fn layout(kind: &BlockKind, bytes: &[u8]) -> Option<Layout> {
    let ls = line_starts(bytes);
    // Line `i` is bytes[ls[i]..ls[i + 1]] (the last runs to the end).
    let mut lines: Vec<Range<usize>> = ls
        .windows(2)
        .map(|w| w[0]..w[1])
        .chain(std::iter::once(ls[ls.len() - 1]..bytes.len()))
        .collect();
    if lines.last().is_some_and(|r| r.is_empty()) {
        lines.pop();
    }
    match kind {
        BlockKind::Table => {
            if lines.len() < 2 + TABLE_MIN_ROWS {
                return None;
            }
            let head = bytes[..lines[1].end].to_vec();
            let widths = column_widths(bytes, &lines);
            let mut seed = Xxh3::new();
            seed.update(&head);
            for w in &widths {
                seed.update(&w.to_le_bytes());
            }
            let pieces = cut(bytes, &lines[2..], seed.digest());
            Some(Layout {
                table: true,
                head,
                tail: Vec::new(),
                widths,
                pieces: with_block_range(pieces, bytes.len(), 2),
            })
        }
        BlockKind::CodeBlock { fenced: true, .. } => {
            let (fence_char, fence_len) = fence(&bytes[lines[0].clone()])?;
            let closed = lines.len() >= 2
                && closes(
                    &bytes[lines[lines.len() - 1].clone()],
                    fence_char,
                    fence_len,
                );
            let body_end = if closed { lines.len() - 1 } else { lines.len() };
            if body_end.saturating_sub(1) < CODE_MIN_LINES {
                return None;
            }
            let head = bytes[lines[0].clone()].to_vec();
            let mut tail = vec![fence_char; fence_len];
            tail.push(b'\n');
            let seed = xxh3_64(&head);
            let pieces = cut(bytes, &lines[1..body_end], seed);
            Some(Layout {
                table: false,
                head,
                tail,
                widths: Vec::new(),
                pieces: with_block_range(pieces, bytes.len(), 1),
            })
        }
        _ => None,
    }
}

/// Splits `rows` into runs by content; ids from the seed and the run's bytes.
fn cut(bytes: &[u8], rows: &[Range<usize>], seed: u64) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, r) in rows.iter().enumerate() {
        let run = i + 1 - from;
        let last = i + 1 == rows.len();
        let hit = xxh3_64(&bytes[r.clone()]).is_multiple_of(AVG_RUN);
        if last || run >= MAX_RUN || (run >= MIN_RUN && hit) {
            let body = rows[from].start..r.end;
            let mut h = Xxh3::new();
            h.update(&seed.to_le_bytes());
            h.update(&bytes[body.clone()]);
            out.push(Piece {
                id: PIECE_ID_BIT | (h.digest() & (PIECE_ID_BIT - 1)),
                start: body.start as u64,
                end: body.end as u64,
                body,
                lines: run as u32,
            });
            from = i + 1;
        }
    }
    out
}

/// The first piece starts at the block's start (it shows the head), the last
/// ends at the block's end (a closing fence, trailing lines).
fn with_block_range(mut pieces: Vec<Piece>, len: usize, head_lines: u32) -> Vec<Piece> {
    if let Some(p) = pieces.first_mut() {
        p.lines += head_lines;
        p.start = 0;
    }
    if let Some(p) = pieces.last_mut() {
        p.end = len as u64;
    }
    pieces
}

/// An opening fence: up to 3 spaces, then 3 or more backticks or tildes.
fn fence(line: &[u8]) -> Option<(u8, usize)> {
    let indent = line.iter().take_while(|&&c| c == b' ').count();
    if indent > 3 {
        return None;
    }
    let c = *line.get(indent)?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = line[indent..].iter().take_while(|&&x| x == c).count();
    (n >= 3).then_some((c, n))
}

fn closes(line: &[u8], c: u8, n: usize) -> bool {
    let t = line.trim_ascii();
    t.len() >= n && t.iter().all(|&x| x == c)
}

/// Column widths in percent, from the widest cell of each column (header
/// and body rows; characters, East Asian wide ones counted twice).
fn column_widths(bytes: &[u8], lines: &[Range<usize>]) -> Vec<f32> {
    let header = cells(&bytes[lines[0].clone()]);
    let mut widest: Vec<usize> = header.iter().map(|c| display_width(c)).collect();
    for r in &lines[2..] {
        for (k, c) in cells(&bytes[r.clone()])
            .iter()
            .enumerate()
            .take(widest.len())
        {
            widest[k] = widest[k].max(display_width(c));
        }
    }
    // A very long cell wraps rather than squeezing the others.
    let w: Vec<f32> = widest.iter().map(|&x| x.clamp(3, 60) as f32).collect();
    let sum: f32 = w.iter().sum::<f32>().max(1.0);
    w.iter().map(|x| x * 100.0 / sum).collect()
}

/// A table row's cells (outer pipes dropped, `\|` not a separator).
fn cells(line: &[u8]) -> Vec<&[u8]> {
    let mut t = line.trim_ascii();
    if t.first() == Some(&b'|') {
        t = &t[1..];
    }
    if t.last() == Some(&b'|') && !t.ends_with(b"\\|") {
        t = &t[..t.len() - 1];
    }
    let mut out = Vec::new();
    let mut from = 0;
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'\\' => i += 1,
            b'|' => {
                out.push(t[from..i].trim_ascii());
                from = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(t[from.min(t.len())..].trim_ascii());
    out
}

fn display_width(cell: &[u8]) -> usize {
    String::from_utf8_lossy(cell)
        .chars()
        .map(|c| if is_wide(c) { 2 } else { 1 })
        .sum()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: usize) -> String {
        let mut s = String::from("| A | Bee | C |\n|:--|:-:|--:|\n");
        for i in 0..rows {
            s.push_str(&format!("| r{i} | 한국어 {i} | {} |\n", i * 7));
        }
        s
    }

    #[test]
    fn small_blocks_stay_whole() {
        assert!(layout(&BlockKind::Table, table(TABLE_MIN_ROWS - 1).as_bytes()).is_none());
        assert!(layout(&BlockKind::Paragraph, table(500).as_bytes()).is_none());
    }

    #[test]
    fn table_pieces_cover_the_block_and_render_the_header() {
        let t = table(1500);
        let l = layout(&BlockKind::Table, t.as_bytes()).unwrap();
        assert!(
            l.pieces.len() >= 15 && l.pieces.len() <= 90,
            "{}",
            l.pieces.len()
        );
        assert_eq!(l.pieces[0].start, 0);
        assert_eq!(l.pieces.last().unwrap().end, t.len() as u64);
        for w in l.pieces.windows(2) {
            assert_eq!(w[0].end, w[1].start, "contiguous");
        }
        let rows: u32 = l.pieces.iter().map(|p| p.lines).sum();
        assert_eq!(rows, 1502);
        let src = String::from_utf8(l.source(&t.as_bytes()[l.pieces[3].body.clone()])).unwrap();
        assert!(src.starts_with("| A | Bee | C |\n|:--|:-:|--:|\n| r"));
        let html = l.wrap(
            3,
            "<table data-sourcepos=\"1:1\">\n<thead></thead></table>\n",
        );
        assert!(html.starts_with(
            "<div class=\"md-piece\"><table class=\"md-piece-table\" data-sourcepos=\"1:1\"><colgroup><col width=\""
        ));
        assert!(
            l.wrap(1, "<p>x</p>")
                .starts_with("<div class=\"md-piece\"><p>")
        );
        assert_eq!(html.matches("<col ").count(), 3);
        assert!(l.wrap(0, "<table></table>").contains("md-piece-first"));
        let sum: f32 = l.widths.iter().sum();
        assert!((sum - 100.0).abs() < 0.1);
        let ids: std::collections::HashSet<u64> = l.pieces.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), l.pieces.len());
        assert!(
            ids.iter()
                .all(|&id| id & PIECE_ID_BIT != 0 && id < (1 << 53))
        );
        assert_eq!(l.piece_at(l.pieces[5].start + 1), 5);
    }

    #[test]
    fn an_edit_changes_only_its_piece() {
        let t = table(1500);
        let a = layout(&BlockKind::Table, t.as_bytes()).unwrap();
        // Insert a row in the middle (same column widths) and edit a cell.
        let at = t.find("| r700 |").unwrap();
        let mut u = t.clone();
        u.insert_str(at, "| new | x | 1 |\n");
        let b = layout(&BlockKind::Table, u.as_bytes()).unwrap();
        let old: std::collections::HashSet<u64> = a.pieces.iter().map(|p| p.id).collect();
        let changed = b.pieces.iter().filter(|p| !old.contains(&p.id)).count();
        assert!(changed <= 2, "{changed} pieces changed");
        let mut v = t.clone();
        v.replace_range(at + 3..at + 4, "R");
        let c = layout(&BlockKind::Table, v.as_bytes()).unwrap();
        let changed = c.pieces.iter().filter(|p| !old.contains(&p.id)).count();
        assert!(changed <= 2, "{changed} pieces changed");
    }

    #[test]
    fn code_pieces_repeat_the_fences() {
        let mut s = String::from("```rust\n");
        for i in 0..1000 {
            s.push_str(&format!("let x{i} = {i};\n"));
        }
        s.push_str("```\n");
        let l = layout(
            &BlockKind::CodeBlock {
                fenced: true,
                info: "rust".into(),
            },
            s.as_bytes(),
        )
        .unwrap();
        assert!(l.pieces.len() >= 10);
        let src = String::from_utf8(l.source(&s.as_bytes()[l.pieces[2].body.clone()])).unwrap();
        assert!(src.starts_with("```rust\nlet x"));
        assert!(src.ends_with("\n```\n"));
        assert_eq!(l.pieces.last().unwrap().end, s.len() as u64);
        // Unclosed: the fence is still added.
        let open = &s[..s.len() - 4];
        let l2 = layout(
            &BlockKind::CodeBlock {
                fenced: true,
                info: String::new(),
            },
            open.as_bytes(),
        )
        .unwrap();
        let last = l2.pieces.len() - 1;
        assert!(
            String::from_utf8(l2.source(&open.as_bytes()[l2.pieces[last].body.clone()]))
                .unwrap()
                .ends_with("```\n")
        );
    }

    #[test]
    fn cells_split_on_unescaped_pipes() {
        let c = cells(b"| a | b \\| c | d |\n");
        assert_eq!(c, vec![&b"a"[..], &b"b \\| c"[..], &b"d"[..]]);
        assert_eq!(cells(b"a|b"), vec![&b"a"[..], &b"b"[..]]);
    }
}
