//! The editable text document (§3.1 TXT, §5.7): the buffer, multi-cursor
//! selections, caret movement, edits as transactions, undo/redo, and the
//! records the recovery journal stores.
//!
//! All offsets are byte offsets into the UTF-8 document. Carets never sit
//! inside a character or between the CR and LF of a CRLF.

use std::io;
use std::ops::ControlFlow;
use std::time::Instant;

use encoding_rs::{Encoding, UTF_8};
use serde::{Deserialize, Serialize};

use crate::buffer::{Snapshot, TextBuffer};
use crate::chars::{Class, chars, class, next_grapheme, prev_grapheme};
use crate::history::{Change, History, Kind, Op, Transaction};
use crate::layout::{
    Layout, VisualRow, column_in_row, line_bounds, offset_at_column, row_after, row_before,
    row_containing,
};
use crate::tree::PieceTree;

/// A selection: `anchor` stays put while `head` (the caret) moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: u64,
    pub head: u64,
    /// Visual column kept across vertical moves.
    #[serde(skip)]
    pub goal: Option<u32>,
}

impl Selection {
    pub fn caret(at: u64) -> Self {
        Self {
            anchor: at,
            head: at,
            goal: None,
        }
    }

    pub fn new(anchor: u64, head: u64) -> Self {
        Self {
            anchor,
            head,
            goal: None,
        }
    }

    pub fn start(&self) -> u64 {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> u64 {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

impl LineEnding {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            LineEnding::Lf => b"\n",
            LineEnding::Crlf => b"\r\n",
            LineEnding::Cr => b"\r",
        }
    }
}

/// How the document is written on save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveFormat {
    pub encoding: &'static Encoding,
    pub bom: bool,
    /// Convert every line break to this on save; `None` keeps them as they are.
    pub line_ending: Option<LineEnding>,
}

impl Default for SaveFormat {
    fn default() -> Self {
        Self {
            encoding: UTF_8,
            bom: false,
            line_ending: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Up,
    Down,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

/// Edit commands from the view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Move {
        to: Move,
        extend: bool,
    },
    /// Typed text (IME commits included).
    Type {
        text: String,
    },
    /// Pasted text: one line per caret when the counts match.
    Paste {
        text: String,
    },
    Newline,
    Tab,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    Undo,
    Redo,
    SelectAll,
    SelectLine,
    AddCursorAbove,
    AddCursorBelow,
    AddNextOccurrence,
    Collapse,
    /// Click or drag: `mode` is "set", "add" or "extend"; `unit` is "char",
    /// "word" or "line".
    Click {
        offset: u64,
        mode: ClickMode,
        unit: ClickUnit,
    },
    /// Box selection over lines by line-relative column.
    ColumnSelect {
        from_line: u64,
        from_col: u32,
        to_line: u64,
        to_col: u32,
    },
    SetSelections {
        ranges: Vec<(u64, u64)>,
    },
    Cut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClickMode {
    Set,
    Add,
    Extend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClickUnit {
    Char,
    Word,
    Line,
}

/// What the journal records for one change, enough to replay it exactly
/// (including undo grouping) on recovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Edit {
        /// `(at, removed length, inserted bytes)` in the order applied.
        ops: Vec<(u64, u64, Vec<u8>)>,
        kind: u8,
        merged: bool,
        selections: Vec<(u64, u64)>,
    },
    Undo,
    Redo,
    Format {
        encoding: String,
        bom: bool,
        line_ending: Option<LineEnding>,
    },
    /// A replace-all: replaying the query on the same content gives the
    /// same result.
    ReplaceAll {
        query: crate::search::Query,
        replacement: String,
    },
}

/// The range a deletion covers from an empty selection at an offset.
type RangeAt<'a> = dyn Fn(&Snapshot, u64) -> io::Result<(u64, u64)> + 'a;

/// Largest selection copied to the clipboard.
pub const MAX_COPY: u64 = 128 << 20;
/// Most cursors a single command creates.
pub const MAX_CURSORS: usize = 100_000;
/// Rows a page move covers when the view hasn't said.
pub const DEFAULT_PAGE_ROWS: u32 = 40;
/// How far word and occurrence scans read before giving up.
const WORD_SCAN: u64 = 64 * 1024;

#[derive(Debug)]
pub struct TextDocument {
    buf: TextBuffer,
    sels: Vec<Selection>,
    primary: usize,
    history: History,
    pub layout: Layout,
    pub page_rows: u32,
    revision: u64,
    saved_top: u64,
    /// Line ending inserted by Enter: the file's dominant one.
    pub eol: LineEnding,
    pub format: SaveFormat,
    saved_format: SaveFormat,
    pub read_only: bool,
    pending: Vec<Record>,
    /// Where large derived buffers (replace-all results) are written.
    pub temp_dir: std::path::PathBuf,
}

fn kind_code(k: Kind) -> u8 {
    match k {
        Kind::Typing => 1,
        Kind::Deleting => 2,
        Kind::Other => 0,
    }
}

fn kind_of(code: u8) -> Kind {
    match code {
        1 => Kind::Typing,
        2 => Kind::Deleting,
        _ => Kind::Other,
    }
}

/// Result of an action.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The text changed.
    pub changed: bool,
    /// Text for the clipboard (cut).
    pub clipboard: Option<String>,
}

/// Moves `at` back onto a character boundary, and off the LF of a CRLF.
pub fn snap_offset(snap: &Snapshot, at: u64) -> io::Result<u64> {
    let len = snap.len();
    let mut at = at.min(len);
    if at == 0 || at == len {
        return Ok(at);
    }
    // A character is at most 4 bytes, so decoding from 4 bytes back
    // resynchronizes before the character holding `at`.
    let from = at.saturating_sub(4);
    let bytes = snap.read(from..(at + 4).min(len))?;
    let rel = (at - from) as usize;
    let start = chars(&bytes)
        .map(|(off, _, _)| off)
        .take_while(|&off| off <= rel)
        .last()
        .unwrap_or(rel);
    at = from + start as u64;
    if at > 0 && at < len && snap.read(at - 1..at + 1)? == b"\r\n" {
        at -= 1;
    }
    Ok(at)
}

impl TextDocument {
    pub fn new(buf: TextBuffer, format: SaveFormat, eol: LineEnding) -> Self {
        Self {
            buf,
            sels: vec![Selection::caret(0)],
            primary: 0,
            history: History::default(),
            layout: Layout::default(),
            page_rows: DEFAULT_PAGE_ROWS,
            revision: 0,
            saved_top: 0,
            eol,
            format,
            saved_format: format,
            read_only: false,
            pending: Vec::new(),
            temp_dir: std::env::temp_dir(),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::new(
            TextBuffer::from_bytes(bytes),
            SaveFormat::default(),
            LineEnding::Lf,
        )
    }

    pub fn buffer(&self) -> &TextBuffer {
        &self.buf
    }

    pub fn buffer_mut(&mut self) -> &mut TextBuffer {
        &mut self.buf
    }

    pub fn snapshot(&self) -> Snapshot {
        self.buf.snapshot()
    }

    pub fn len(&self) -> u64 {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn selections(&self) -> &[Selection] {
        &self.sels
    }

    pub fn primary(&self) -> Selection {
        self.sels[self.primary]
    }

    pub fn is_dirty(&self) -> bool {
        self.history.top() != self.saved_top || self.format != self.saved_format
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// Marks the current state as saved.
    pub fn mark_saved(&mut self) {
        self.saved_top = self.history.top();
        self.saved_format = self.format;
    }

    /// Journal records produced since the last call.
    pub fn take_records(&mut self) -> Vec<Record> {
        std::mem::take(&mut self.pending)
    }

    /// Text of `range` as UTF-8 (lossy for invalid bytes).
    pub fn text(&self, from: u64, to: u64) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.snapshot().read(from..to)?).into_owned())
    }

    // ---- selections ------------------------------------------------------

    /// Sorts and merges overlapping selections; keeps track of the primary.
    fn normalize(&mut self) {
        let len = self.len();
        let primary = self.sels[self.primary.min(self.sels.len() - 1)];
        for s in &mut self.sels {
            s.anchor = s.anchor.min(len);
            s.head = s.head.min(len);
        }
        self.sels.sort_by_key(|s| (s.start(), s.end()));
        let mut out: Vec<Selection> = Vec::with_capacity(self.sels.len());
        let mut primary_at = 0;
        for s in self.sels.drain(..) {
            if let Some(last) = out.last_mut() {
                let overlap = s.start() < last.end() || s.start() == last.start();
                if overlap {
                    let (a, b) = (last.start().min(s.start()), last.end().max(s.end()));
                    let forward = last.head >= last.anchor;
                    *last = if forward {
                        Selection::new(a, b)
                    } else {
                        Selection::new(b, a)
                    };
                    if s == primary {
                        primary_at = out.len() - 1;
                    }
                    continue;
                }
            }
            if s == primary {
                primary_at = out.len();
            }
            out.push(s);
        }
        self.sels = out;
        self.primary = primary_at.min(self.sels.len() - 1);
    }

    pub fn set_selections(&mut self, sels: Vec<Selection>, primary: usize) {
        if sels.is_empty() {
            return;
        }
        self.primary = primary.min(sels.len() - 1);
        self.sels = sels;
        self.normalize();
    }

    // ---- reading around offsets -------------------------------------------

    fn window(&self, snap: &Snapshot, from: u64, to: u64) -> io::Result<(u64, Vec<u8>)> {
        let to = to.min(snap.len());
        let from = from.min(to);
        Ok((from, snap.read(from..to)?))
    }

    fn next_boundary(&self, snap: &Snapshot, at: u64) -> io::Result<u64> {
        if at >= snap.len() {
            return Ok(snap.len());
        }
        let (start, bytes) = self.window(snap, at, at + 256)?;
        Ok(start + next_grapheme(&bytes, 0) as u64)
    }

    fn prev_boundary(&self, snap: &Snapshot, at: u64) -> io::Result<u64> {
        if at == 0 {
            return Ok(0);
        }
        let (start, bytes) = self.window(snap, at.saturating_sub(256), at)?;
        Ok(start + prev_grapheme(&bytes, bytes.len()) as u64)
    }

    /// End of the word (or punctuation run) after `at`, VS Code style:
    /// skip spaces, then one run of a class. A line break is its own stop.
    fn word_right(&self, snap: &Snapshot, at: u64) -> io::Result<u64> {
        let (start, bytes) = self.window(snap, at, at + WORD_SCAN)?;
        let mut it = chars(&bytes).peekable();
        let Some(&(_, _, c0)) = it.peek() else {
            return Ok(at);
        };
        if class(c0) == Class::Break {
            return self.next_boundary(snap, at);
        }
        let mut run: Option<Class> = None;
        for (off, _, c) in it {
            let k = class(c);
            match (run, k) {
                (_, Class::Break) => return Ok(start + off as u64),
                (None, Class::Space) => {}
                (None, k) => run = Some(k),
                (Some(Class::Wide), _) => return Ok(start + off as u64),
                (Some(r), k) if r != k => return Ok(start + off as u64),
                _ => {}
            }
        }
        Ok(start + bytes.len() as u64)
    }

    fn word_left(&self, snap: &Snapshot, at: u64) -> io::Result<u64> {
        let (start, bytes) = self.window(snap, at.saturating_sub(WORD_SCAN), at)?;
        let cs: Vec<_> = chars(&bytes).collect();
        let Some(&(_, _, last)) = cs.last() else {
            return Ok(at);
        };
        if class(last) == Class::Break {
            return self.prev_boundary(snap, at);
        }
        let mut run: Option<Class> = None;
        for &(off, n, c) in cs.iter().rev() {
            let k = class(c);
            match (run, k) {
                (_, Class::Break) => return Ok(start + (off + n) as u64),
                (None, Class::Space) => {}
                (None, k) => run = Some(k),
                (Some(Class::Wide), _) => return Ok(start + (off + n) as u64),
                (Some(r), k) if r != k => return Ok(start + (off + n) as u64),
                _ => {}
            }
        }
        Ok(start)
    }

    /// The word around `at` (for double-click and add-next-occurrence).
    fn word_at(&self, snap: &Snapshot, at: u64) -> io::Result<(u64, u64)> {
        let (start, bytes) = self.window(snap, at.saturating_sub(WORD_SCAN), at + WORD_SCAN)?;
        let rel = (at - start) as usize;
        let cs: Vec<_> = chars(&bytes).collect();
        let i = cs.partition_point(|&(off, _, _)| off < rel);
        let target = cs
            .get(i)
            .or_else(|| i.checked_sub(1).and_then(|j| cs.get(j)));
        let Some(&(_, _, c)) = target else {
            return Ok((at, at));
        };
        let k = class(c);
        if matches!(k, Class::Break | Class::Space) && i > 0 && class(cs[i - 1].2) == Class::Word {
            return self.word_at(snap, at - cs[i - 1].1 as u64);
        }
        let mut a = i.min(cs.len() - 1);
        while a > 0 && class(cs[a - 1].2) == k && k != Class::Wide {
            a -= 1;
        }
        let mut b = i.min(cs.len() - 1);
        while b < cs.len() && class(cs[b].2) == k && (k != Class::Wide || b == i) {
            b += 1;
        }
        let end = cs.get(b).map_or(bytes.len(), |c| c.0);
        Ok((start + cs[a].0 as u64, start + end as u64))
    }

    fn smart_home(&self, snap: &Snapshot, row: &VisualRow, head: u64) -> io::Result<u64> {
        if !row.is_first() {
            return Ok(row.start);
        }
        let (start, bytes) = self.window(snap, row.start, row.end.min(row.start + WORD_SCAN))?;
        let indent = bytes
            .iter()
            .position(|&b| b != b' ' && b != b'\t')
            .unwrap_or(bytes.len());
        let first = start + indent as u64;
        Ok(if head == first { row.start } else { first })
    }

    fn move_head(
        &self,
        snap: &Snapshot,
        s: Selection,
        to: Move,
        extend: bool,
    ) -> io::Result<Selection> {
        let layout = self.layout;
        let h = s.head;
        let collapse = !extend && !s.is_empty();
        let (head, goal) = match to {
            Move::Left if collapse => (s.start(), None),
            Move::Right if collapse => (s.end(), None),
            Move::Left => (self.prev_boundary(snap, h)?, None),
            Move::Right => (self.next_boundary(snap, h)?, None),
            Move::WordLeft => (self.word_left(snap, h)?, None),
            Move::WordRight => (self.word_right(snap, h)?, None),
            Move::Home => {
                let row = row_containing(snap, h, &layout)?;
                (self.smart_home(snap, &row, h)?, None)
            }
            Move::End => {
                let row = row_containing(snap, h, &layout)?;
                (row.end, None)
            }
            Move::DocStart => (0, None),
            Move::DocEnd => (snap.len(), None),
            Move::Up | Move::Down | Move::PageUp | Move::PageDown => {
                let n = match to {
                    Move::Up | Move::Down => 1,
                    _ => self.page_rows.max(1),
                };
                let down = matches!(to, Move::Down | Move::PageDown);
                let mut row = row_containing(snap, h, &layout)?;
                let goal = match s.goal {
                    Some(g) => g,
                    None => column_in_row(snap, &row, h, layout.tab)?,
                };
                let mut moved = 0;
                for _ in 0..n {
                    let next = if down {
                        row_after(snap, &row, &layout)?
                    } else {
                        row_before(snap, &row, &layout)?
                    };
                    match next {
                        Some(r) => {
                            row = r;
                            moved += 1;
                        }
                        None => break,
                    }
                }
                if moved == 0 {
                    (if down { snap.len() } else { 0 }, Some(goal))
                } else {
                    let mut at = offset_at_column(snap, &row, goal, layout.tab)?;
                    // A wrapped row's end is the next row's start: stay on this row.
                    if at == row.end && row.end < row.line_end && at > row.start {
                        at = self.prev_boundary(snap, at)?;
                    }
                    (at, Some(goal))
                }
            }
        };
        Ok(Selection {
            anchor: if extend { s.anchor } else { head },
            head,
            goal,
        })
    }

    // ---- edits ------------------------------------------------------------

    /// Replaces each `(from, to, text)` (sorted, non-overlapping) as one
    /// transaction; carets go to the end of each inserted text.
    fn replace_ranges(&mut self, edits: Vec<(u64, u64, Vec<u8>)>, kind: Kind) -> io::Result<bool> {
        if edits.is_empty() || self.read_only {
            return Ok(false);
        }
        let sel_before = self.sels.clone();
        let mut ops = Vec::with_capacity(edits.len());
        let mut rec_ops = Vec::with_capacity(edits.len());
        for (from, to, text) in edits.iter().rev() {
            let r = self.buf.replace(*from, *to, text)?;
            rec_ops.push((*from, to - from, text.clone()));
            ops.push(Op {
                at: *from,
                removed: r.pieces,
                inserted: r.inserted,
            });
        }
        let mut shift: i64 = 0;
        let mut sels = Vec::with_capacity(edits.len());
        for (from, to, text) in &edits {
            let caret = (*from as i64 + shift) as u64 + text.len() as u64;
            sels.push(Selection::caret(caret));
            shift += text.len() as i64 - (to - from) as i64;
        }
        let primary = self.primary.min(sels.len() - 1);
        self.sels = sels;
        self.primary = primary;
        self.normalize();
        let tx = Transaction {
            id: self.history.new_id(),
            change: Change::Ops(ops),
            sel_before,
            sel_after: self.sels.clone(),
            kind,
            at: Instant::now(),
        };
        let merged = self.history.push(tx);
        self.pending.push(Record::Edit {
            ops: rec_ops,
            kind: kind_code(kind),
            merged,
            selections: self.sels.iter().map(|s| (s.anchor, s.head)).collect(),
        });
        self.revision += 1;
        Ok(true)
    }

    /// Ranges each selection affects for a deletion: the selection itself,
    /// or `f(caret)` when it's empty. Overlaps are merged.
    fn deletion_ranges(&self, f: &RangeAt<'_>) -> io::Result<Vec<(u64, u64, Vec<u8>)>> {
        let snap = self.snapshot();
        let mut out: Vec<(u64, u64, Vec<u8>)> = Vec::new();
        for s in &self.sels {
            let (a, b) = if s.is_empty() {
                f(&snap, s.head)?
            } else {
                (s.start(), s.end())
            };
            if a == b {
                continue;
            }
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b, Vec::new())),
            }
        }
        Ok(out)
    }

    fn insert_at_selections(&mut self, texts: &[Vec<u8>], kind: Kind) -> io::Result<bool> {
        let edits: Vec<_> = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| (s.start(), s.end(), texts[i % texts.len()].clone()))
            .collect();
        self.replace_ranges(edits, kind)
    }

    fn selected_text(&self) -> io::Result<Option<String>> {
        let total: u64 = self.sels.iter().map(|s| s.end() - s.start()).sum();
        if total > MAX_COPY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("selection too large to copy ({total} bytes; limit {MAX_COPY})"),
            ));
        }
        if self.sels.iter().all(Selection::is_empty) {
            return Ok(None);
        }
        let snap = self.snapshot();
        let mut parts = Vec::new();
        for s in &self.sels {
            if !s.is_empty() {
                parts.push(String::from_utf8_lossy(&snap.read(s.start()..s.end())?).into_owned());
            }
        }
        let sep = String::from_utf8_lossy(self.eol.bytes()).into_owned();
        Ok(Some(parts.join(&sep)))
    }

    /// Text to copy, or `None` with no selection. Fails above [`MAX_COPY`].
    pub fn copy(&self) -> io::Result<Option<String>> {
        self.selected_text()
    }

    fn undo(&mut self) -> io::Result<bool> {
        let Some(tx) = self.history.pop_undo() else {
            return Ok(false);
        };
        match &tx.change {
            Change::Ops(ops) => {
                for op in ops.iter().rev() {
                    self.buf
                        .replace_pieces(op.at, op.at + op.inserted.len(), &op.removed)?;
                }
            }
            Change::Tree { before, .. } => self.buf.set_tree(before.clone()),
            Change::Format { before, .. } => self.format = *before,
        }
        self.sels = tx.sel_before.clone();
        self.primary = self.sels.len() - 1;
        self.normalize();
        self.history.push_redo(tx);
        self.pending.push(Record::Undo);
        self.revision += 1;
        Ok(true)
    }

    fn redo(&mut self) -> io::Result<bool> {
        let Some(tx) = self.history.pop_redo() else {
            return Ok(false);
        };
        match &tx.change {
            Change::Ops(ops) => {
                for op in ops {
                    self.buf
                        .replace_pieces(op.at, op.at + op.removed.len(), &op.inserted)?;
                }
            }
            Change::Tree { after, .. } => self.buf.set_tree(after.clone()),
            Change::Format { after, .. } => self.format = *after,
        }
        self.sels = tx.sel_after.clone();
        self.primary = self.sels.len() - 1;
        self.normalize();
        self.history.push_undone(tx);
        self.pending.push(Record::Redo);
        self.revision += 1;
        Ok(true)
    }

    /// Replaces the whole tree as one undoable step (replace-all).
    pub fn swap_tree(&mut self, after: PieceTree, record: Record) {
        let before = self.buf.tree().clone();
        let sel_before = self.sels.clone();
        self.buf.set_tree(after.clone());
        self.sels = vec![Selection::caret(self.primary().head.min(self.len()))];
        self.primary = 0;
        let tx = Transaction {
            id: self.history.new_id(),
            change: Change::Tree { before, after },
            sel_before,
            sel_after: self.sels.clone(),
            kind: Kind::Other,
            at: Instant::now(),
        };
        self.history.push(tx);
        self.pending.push(record);
        self.revision += 1;
    }

    /// Changes the save format as an undoable step.
    pub fn set_format(&mut self, format: SaveFormat) {
        if format == self.format {
            return;
        }
        let before = self.format;
        self.format = format;
        let tx = Transaction {
            id: self.history.new_id(),
            change: Change::Format {
                before,
                after: format,
            },
            sel_before: self.sels.clone(),
            sel_after: self.sels.clone(),
            kind: Kind::Other,
            at: Instant::now(),
        };
        self.history.push(tx);
        self.pending.push(Record::Format {
            encoding: format.encoding.name().to_owned(),
            bom: format.bom,
            line_ending: format.line_ending,
        });
        self.revision += 1;
    }

    /// Replays a journal record (recovery). Grouping follows the record.
    pub fn apply_record(&mut self, r: &Record) -> io::Result<()> {
        match r {
            Record::Edit {
                ops,
                kind,
                merged,
                selections,
            } => {
                let sel_before = self.sels.clone();
                let mut tops = Vec::new();
                for (at, del, ins) in ops {
                    let rr = self.buf.replace(*at, at + del, ins)?;
                    tops.push(Op {
                        at: *at,
                        removed: rr.pieces,
                        inserted: rr.inserted,
                    });
                }
                let sels: Vec<Selection> = selections
                    .iter()
                    .map(|&(a, h)| Selection::new(a, h))
                    .collect();
                if !sels.is_empty() {
                    self.sels = sels;
                    self.primary = self.sels.len() - 1;
                    self.normalize();
                }
                let tx = Transaction {
                    id: self.history.new_id(),
                    change: Change::Ops(tops),
                    sel_before,
                    sel_after: self.sels.clone(),
                    kind: kind_of(*kind),
                    at: Instant::now(),
                };
                self.history.push_forced(tx, *merged);
                self.revision += 1;
            }
            Record::Undo => {
                self.undo()?;
            }
            Record::Redo => {
                self.redo()?;
            }
            Record::Format {
                encoding,
                bom,
                line_ending,
            } => {
                let encoding = Encoding::for_label(encoding.as_bytes()).unwrap_or(UTF_8);
                self.set_format(SaveFormat {
                    encoding,
                    bom: *bom,
                    line_ending: *line_ending,
                });
            }
            Record::ReplaceAll { query, replacement } => {
                let m = crate::search::Matcher::new(query).map_err(io::Error::other)?;
                let snap = self.snapshot();
                let adder = self.buf.adder();
                let cancel = quire_core::scheduler::CancelToken::new();
                if let Some(r) = crate::replace::compute(
                    &snap,
                    &adder,
                    &m,
                    replacement,
                    &self.temp_dir,
                    &cancel,
                    &|_| {},
                )? {
                    self.swap_tree(
                        r.tree,
                        Record::ReplaceAll {
                            query: query.clone(),
                            replacement: replacement.clone(),
                        },
                    );
                }
            }
        }
        self.pending.clear();
        Ok(())
    }

    // ---- actions ----------------------------------------------------------

    pub fn apply(&mut self, action: &Action) -> io::Result<Outcome> {
        let snap = self.snapshot();
        let mut out = Outcome::default();
        match action {
            Action::Move { to, extend } => {
                let mut next = Vec::with_capacity(self.sels.len());
                for s in &self.sels {
                    next.push(self.move_head(&snap, *s, *to, *extend)?);
                }
                self.sels = next;
                self.normalize();
            }
            Action::Type { text } => {
                let kind = if text.chars().all(|c| !c.is_whitespace() || c == ' ') {
                    Kind::Typing
                } else {
                    Kind::Other
                };
                out.changed = self.insert_at_selections(&[text.clone().into_bytes()], kind)?;
            }
            Action::Paste { text } => {
                let lines: Vec<&str> = text.split_inclusive('\n').collect();
                let texts: Vec<Vec<u8>> = if self.sels.len() > 1 && lines.len() == self.sels.len() {
                    lines
                        .iter()
                        .map(|l| l.trim_end_matches(['\r', '\n']).as_bytes().to_vec())
                        .collect()
                } else {
                    vec![text.clone().into_bytes()]
                };
                out.changed = self.insert_at_selections(&texts, Kind::Other)?;
            }
            Action::Newline => {
                out.changed =
                    self.insert_at_selections(&[self.eol.bytes().to_vec()], Kind::Other)?;
            }
            Action::Tab => {
                out.changed = self.insert_at_selections(&[b"\t".to_vec()], Kind::Typing)?;
            }
            Action::Backspace
            | Action::Delete
            | Action::DeleteWordLeft
            | Action::DeleteWordRight => {
                let edits = self.deletion_ranges(&|snap, at| {
                    Ok(match action {
                        Action::Backspace => (self.prev_boundary(snap, at)?, at),
                        Action::Delete => (at, self.next_boundary(snap, at)?),
                        Action::DeleteWordLeft => (self.word_left(snap, at)?, at),
                        _ => (at, self.word_right(snap, at)?),
                    })
                })?;
                out.changed = self.replace_ranges(edits, Kind::Deleting)?;
            }
            Action::Cut => {
                out.clipboard = self.selected_text()?;
                let edits = self.deletion_ranges(&|_, at| Ok((at, at)))?;
                out.changed = self.replace_ranges(edits, Kind::Other)?;
            }
            Action::Undo => out.changed = self.undo()?,
            Action::Redo => out.changed = self.redo()?,
            Action::SelectAll => {
                self.sels = vec![Selection::new(0, self.len())];
                self.primary = 0;
            }
            Action::SelectLine => {
                let mut next = Vec::new();
                for s in &self.sels {
                    let (a, _, _) = line_bounds(&snap, s.start())?;
                    let (_, _, n) = line_bounds(&snap, s.end())?;
                    next.push(Selection::new(a, n));
                }
                self.sels = next;
                self.normalize();
            }
            Action::Collapse => {
                let p = self.primary();
                self.sels = vec![Selection::caret(p.head)];
                self.primary = 0;
            }
            Action::AddCursorAbove | Action::AddCursorBelow => {
                let down = matches!(action, Action::AddCursorBelow);
                let edge = if down {
                    *self.sels.iter().max_by_key(|s| s.head).expect("non-empty")
                } else {
                    *self.sels.iter().min_by_key(|s| s.head).expect("non-empty")
                };
                let to = if down { Move::Down } else { Move::Up };
                let moved = self.move_head(&snap, Selection::caret(edge.head), to, false)?;
                if moved.head != edge.head && self.sels.len() < MAX_CURSORS {
                    self.sels.push(moved);
                    self.primary = self.sels.len() - 1;
                    self.normalize();
                }
            }
            Action::AddNextOccurrence => self.add_next_occurrence(&snap)?,
            Action::Click { offset, mode, unit } => {
                let at = snap_offset(&snap, *offset)?;
                let (a, b) = match unit {
                    ClickUnit::Char => (at, at),
                    ClickUnit::Word => self.word_at(&snap, at)?,
                    ClickUnit::Line => {
                        let (s, _, n) = line_bounds(&snap, at)?;
                        (s, n)
                    }
                };
                match mode {
                    ClickMode::Set => {
                        self.sels = vec![Selection::new(a, b)];
                        self.primary = 0;
                    }
                    ClickMode::Add => {
                        if self.sels.len() < MAX_CURSORS {
                            self.sels.push(Selection::new(a, b));
                            self.primary = self.sels.len() - 1;
                        }
                    }
                    ClickMode::Extend => {
                        let p = self.primary();
                        let head = if b > p.anchor { b } else { a };
                        self.sels = vec![Selection::new(p.anchor, head)];
                        self.primary = 0;
                    }
                }
                self.normalize();
            }
            Action::ColumnSelect {
                from_line,
                from_col,
                to_line,
                to_col,
            } => self.column_select(&snap, *from_line, *from_col, *to_line, *to_col)?,
            Action::SetSelections { ranges } => {
                if !ranges.is_empty() {
                    let mut sels = Vec::with_capacity(ranges.len().min(MAX_CURSORS));
                    for &(a, h) in ranges.iter().take(MAX_CURSORS) {
                        sels.push(Selection::new(
                            snap_offset(&snap, a)?,
                            snap_offset(&snap, h)?,
                        ));
                    }
                    self.sels = sels;
                    self.primary = self.sels.len() - 1;
                    self.normalize();
                }
            }
        }
        Ok(out)
    }

    fn add_next_occurrence(&mut self, snap: &Snapshot) -> io::Result<()> {
        let p = self.primary();
        if p.is_empty() {
            let (a, b) = self.word_at(snap, p.head)?;
            if a < b {
                self.sels[self.primary] = Selection::new(a, b);
            }
            return Ok(());
        }
        let needle = snap.read(p.start()..p.end())?;
        let last_end = self.sels.iter().map(Selection::end).max().unwrap_or(0);
        let finder = memchr::memmem::Finder::new(&needle);
        let overlap = needle.len().saturating_sub(1) as u64;
        let mut found = None;
        for (from, to) in [(last_end, snap.len()), (0, last_end)] {
            let mut at = from;
            while at < to && found.is_none() {
                let end = (at + (8 << 20) + overlap).min(to);
                let block = snap.read(at..end)?;
                let mut pos = 0;
                while let Some(i) = finder.find(&block[pos..]) {
                    let hit = at + (pos + i) as u64;
                    if !self.sels.iter().any(|s| s.start() == hit) {
                        found = Some(hit);
                        break;
                    }
                    pos += i + 1;
                }
                at = end.saturating_sub(overlap).max(at + 1);
                if end == to {
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        if let Some(hit) = found
            && self.sels.len() < MAX_CURSORS
        {
            self.sels
                .push(Selection::new(hit, hit + needle.len() as u64));
            self.primary = self.sels.len() - 1;
            self.normalize();
        }
        Ok(())
    }

    fn column_select(
        &mut self,
        snap: &Snapshot,
        l0: u64,
        c0: u32,
        l1: u64,
        c1: u32,
    ) -> io::Result<()> {
        let (lo, hi) = (l0.min(l1), l0.max(l1));
        let hi = hi.min(lo + MAX_CURSORS as u64 - 1);
        let tab = self.layout.tab;
        let unwrapped = Layout { wrap: None, tab };
        let (mut at, _) = snap.line_start_estimate(lo);
        let mut sels = Vec::new();
        for _ in lo..=hi {
            let row = row_containing(snap, at, &unwrapped)?;
            let a = offset_at_column(snap, &row, c0, tab)?;
            let h = offset_at_column(snap, &row, c1, tab)?;
            sels.push(Selection {
                anchor: a,
                head: h,
                goal: Some(c1),
            });
            if row.next_line <= row.line_end {
                break;
            }
            at = row.next_line;
        }
        if l1 < l0 {
            sels.reverse();
        }
        let primary = sels.len() - 1;
        self.sels = sels;
        self.primary = primary;
        self.normalize();
        Ok(())
    }

    /// Visits selections overlapping `from..to`, for drawing.
    pub fn selections_in(
        &self,
        from: u64,
        to: u64,
        f: &mut dyn FnMut(&Selection, bool) -> ControlFlow<()>,
    ) {
        let i = self.sels.partition_point(|s| s.end() < from);
        for (k, s) in self.sels.iter().enumerate().skip(i) {
            if s.start() > to {
                break;
            }
            if f(s, k == self.primary).is_break() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
