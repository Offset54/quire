//! What the preview shows: windows of rendered blocks around a byte offset,
//! with a render cache that knows when a block must be rendered again
//! (ADR-0030).
//!
//! A block's rendering depends on its bytes (its id changes when they do),
//! the render options (image policy, remote toggle) and a few values from
//! the document-wide tables: the definitions of the labels it references,
//! its footnotes' numbers and reference counts, its headings' ids. The cache
//! keeps a hash of those values (the *signature*) with each rendering; after
//! the tables are rebuilt, a cached block is rendered again only if its
//! signature changed. Each rendering gets a revision number, so the
//! frontend, which keeps blocks by id, can tell a new rendering of the same
//! block from the one it has.
//!
//! Code highlighting is lazy (ADR-0029): renderings take highlighted code
//! from `quire_highlight::service`'s cache, or render it plain and request
//! it. Once the background thread has it, a pass over the shown entries
//! ([`Preview::highlight_due`], [`Highlight::Upgrade`]) renders them again,
//! a few milliseconds at a time.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use xxhash_rust::xxh3::Xxh3;

use crate::analysis::normalize_label;
use crate::document::MarkdownDoc;
use crate::index::Block;
use crate::pieces::{self, Layout};
use crate::render::{RenderOptions, Rendered};
use crate::scan::BlockKind;
use crate::tables::Tables;
use crate::text::{Text, line_starts};

/// Kind codes in window messages (`apps/desktop/src/md/protocol.ts`).
pub fn kind_code(k: &BlockKind) -> u8 {
    match k {
        BlockKind::FrontMatter => 1,
        BlockKind::Paragraph => 2,
        BlockKind::Heading { .. } => 3,
        BlockKind::ThematicBreak => 4,
        BlockKind::CodeBlock { .. } => 5,
        BlockKind::HtmlBlock => 6,
        BlockKind::List => 7,
        BlockKind::BlockQuote => 8,
        BlockKind::Table => 9,
        BlockKind::FootnoteDefinition { .. } => 10,
        BlockKind::Math => 11,
        BlockKind::Definitions => 12,
        BlockKind::Other => 0,
    }
}

/// Detail for a kind: a heading's level, 1 for a fenced code block.
pub fn kind_detail(k: &BlockKind) -> u8 {
    match k {
        BlockKind::Heading { level, .. } => *level,
        BlockKind::CodeBlock { fenced, .. } => u8::from(*fenced),
        _ => 0,
    }
}

/// Blocks larger than this aren't rendered: the preview shows the start of
/// their source, labeled (§2: degrade honestly). The source view edits
/// them as usual.
pub const MAX_RENDER_BYTES: u64 = 512 * 1024;
/// Source shown for such a block.
const LARGE_PREFIX: usize = 16 * 1024;
/// Pieces of a large block a window shows on each side of the anchor's
/// piece ([`crate::pieces`]).
pub const PIECES_AROUND: usize = 8;

fn render_large(bytes: &[u8], total: u64) -> Rendered {
    let mut cut = LARGE_PREFIX.min(bytes.len());
    while cut > 0 && cut < bytes.len() && (bytes[cut] & 0xc0) == 0x80 {
        cut -= 1;
    }
    let text = String::from_utf8_lossy(&bytes[..cut]);
    let mut html = format!("<div class=\"md-too-large\" data-bytes=\"{total}\"><pre>");
    for c in text.chars() {
        match c {
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '&' => html.push_str("&amp;"),
            c => html.push(c),
        }
    }
    html.push_str("</pre></div>\n");
    Rendered {
        html,
        ..Rendered::default()
    }
}

/// Hash of the table values a rendering used.
pub fn signature(tables: &Tables, block: &Block, r: &Rendered) -> u64 {
    let mut h = Xxh3::new();
    for d in &r.deps {
        h.update(d.as_bytes());
        match tables.definition(d) {
            Some(def) => {
                h.update(b"\x01");
                h.update(def.url.as_bytes());
                h.update(b"\x00");
                h.update(def.title.as_bytes());
            }
            None => h.update(b"\x02"),
        }
    }
    let footnote = |h: &mut Xxh3, key: &str| {
        h.update(key.as_bytes());
        h.update(&[u8::from(tables.footnote_defined(key))]);
        h.update(&tables.footnote_number(key).unwrap_or(0).to_le_bytes());
        h.update(&tables.footnote_refs_before(block.id, key).to_le_bytes());
        h.update(&tables.footnote_refs_total(key).to_le_bytes());
    };
    for f in &r.footnotes {
        footnote(&mut h, f);
    }
    if let BlockKind::FootnoteDefinition { label } = &block.kind {
        footnote(&mut h, &normalize_label(label));
    }
    for i in 0..r.headings {
        h.update(tables.heading_slug(block.id, i).unwrap_or("").as_bytes());
        h.update(b"\x00");
    }
    h.digest()
}

/// Renders an entry: (document, provisional, options) → (rendering, lines).
type MakeRendering<'a> =
    dyn FnMut(&mut MarkdownDoc, bool, &RenderOptions) -> io::Result<(Rendered, u32)> + 'a;

struct Entry {
    rendered: Arc<Rendered>,
    sig: u64,
    tables_gen: u64,
    opts_gen: u64,
    rev: u64,
    lines: u32,
    tick: u64,
}

/// One block of a window.
#[derive(Clone, Debug)]
pub struct WindowBlock {
    pub id: u64,
    pub start: u64,
    pub end: u64,
    pub kind: u8,
    /// Lines in the block (height estimates, scroll sync).
    pub lines: u32,
    /// [`kind_detail`].
    pub detail: u8,
    /// Revision of its rendering.
    pub rev: u64,
    /// The rendering, when the frontend doesn't have this revision.
    pub html: Option<Arc<Rendered>>,
}

#[derive(Clone, Debug, Default)]
pub struct Window {
    /// Index of the document block the window starts in.
    pub first: usize,
    /// Position of the entry holding the requested offset within `blocks`.
    pub anchor: usize,
    /// Entries: blocks, and pieces of large blocks ([`crate::pieces`]).
    pub blocks: Vec<WindowBlock>,
    /// The window holds the document's first entry / its last one (the
    /// index is complete and nothing follows).
    pub at_start: bool,
    pub at_end: bool,
    /// Blocks indexed so far, and where the index ends.
    pub indexed_blocks: usize,
    pub frontier: u64,
    pub complete: bool,
    pub len: u64,
}

/// Highlighting a rendering waited for is ready.
fn highlight_ready(r: &Rendered) -> bool {
    r.highlight_pending
        .iter()
        .any(|k| quire_highlight::service::is_ready(*k))
}

/// How renderings made now treat code in a known language (ADR-0029).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    /// From the highlight cache, else plain and requested (first paint,
    /// scrolling).
    Off,
    /// Small code whose grammar is ready is highlighted right away (an
    /// edited block, so it doesn't flash plain while typing).
    New,
    /// For this long, also render again the cached renderings whose
    /// highlighting is now ready. A rendering started before the end
    /// finishes.
    Upgrade(Duration),
}

/// The preview's render cache.
pub struct Preview {
    /// Renderings by entry id (a block's id, or a piece's).
    entries: HashMap<u64, Entry>,
    /// Piece layouts by block id (None: shown whole).
    layouts: HashMap<u64, Option<Arc<Layout>>>,
    /// Render with the tables as they are, without waiting for a rebuild.
    provisional: bool,
    highlight: Highlight,
    /// End of an [`Highlight::Upgrade`] pass.
    highlight_until: Option<Instant>,
    opts: RenderOptions,
    /// `opts` with highlighting on.
    opts_hl: RenderOptions,
    opts_gen: u64,
    next_rev: u64,
    tick: u64,
    bytes: u64,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preview")
            .field("entries", &self.entries.len())
            .field("bytes", &self.bytes)
            .finish()
    }
}

fn entry_bytes(r: &Rendered) -> u64 {
    (r.html.len() + r.deps.iter().map(String::len).sum::<usize>() + 96) as u64
}

impl Preview {
    pub fn new(opts: RenderOptions) -> Self {
        let (opts, opts_hl) = Self::split(opts);
        Self {
            entries: HashMap::new(),
            layouts: HashMap::new(),
            provisional: false,
            highlight: Highlight::Off,
            highlight_until: None,
            opts,
            opts_hl,
            opts_gen: 1,
            next_rev: 1,
            tick: 0,
            bytes: 0,
        }
    }

    pub fn options(&self) -> &RenderOptions {
        &self.opts
    }

    /// New render options (the remote toggle): every block renders again
    /// when next shown.
    pub fn set_options(&mut self, opts: RenderOptions) {
        (self.opts, self.opts_hl) = Self::split(opts);
        self.opts_gen += 1;
    }

    fn split(opts: RenderOptions) -> (RenderOptions, RenderOptions) {
        let plain = RenderOptions {
            highlight: false,
            ..opts.clone()
        };
        let hl = RenderOptions {
            highlight: true,
            ..opts
        };
        (plain, hl)
    }

    /// How renderings made from now on treat code ([`Highlight`]).
    pub fn set_highlight(&mut self, mode: Highlight) {
        self.highlight = mode;
        self.highlight_until = match mode {
            Highlight::Upgrade(budget) => Some(Instant::now() + budget),
            _ => None,
        };
    }

    /// (highlight new renderings, render pending ones again).
    fn highlighting(&self) -> (bool, bool) {
        match self.highlight {
            Highlight::Off => (false, false),
            Highlight::New => (true, false),
            Highlight::Upgrade(_) => {
                let on = self.highlight_until.is_some_and(|d| Instant::now() < d);
                (false, on)
            }
        }
    }

    /// Whether any of `shown` (entry id, revision) was rendered plain
    /// waiting for highlighting that is now ready.
    pub fn highlight_due(&self, shown: &[(u64, u64)]) -> bool {
        shown.iter().any(|(id, rev)| {
            self.entries
                .get(id)
                .is_some_and(|e| e.rev == *rev && highlight_ready(&e.rendered))
        })
    }

    /// Provisional rendering: while on, blocks render against the tables as
    /// they are, even when an edit left them waiting for a rebuild. Right
    /// after an edit the window is shown this way first (the edited block's
    /// own text and structure don't depend on the tables), then again once
    /// the tables are rebuilt: renderings whose table values changed get a
    /// new revision then (their signatures differ).
    pub fn set_provisional(&mut self, on: bool) {
        self.provisional = on;
    }

    /// Bytes the cache holds (reported to the memory governor).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops renderings of blocks that are gone.
    pub fn forget(&mut self, ids: &[u64]) {
        for id in ids {
            // A large block's pieces age out of the cache like any entry.
            self.layouts.remove(id);
            if let Some(e) = self.entries.remove(id) {
                self.bytes -= entry_bytes(&e.rendered);
            }
        }
    }

    /// Evicts the least recently shown renderings down to `max` bytes;
    /// returns the bytes freed.
    pub fn trim(&mut self, max: u64) -> u64 {
        if self.bytes <= max {
            return 0;
        }
        let before = self.bytes;
        let mut by_age: Vec<(u64, u64)> =
            self.entries.iter().map(|(id, e)| (e.tick, *id)).collect();
        by_age.sort_unstable();
        for (_, id) in by_age {
            if self.bytes <= max {
                break;
            }
            if let Some(e) = self.entries.remove(&id) {
                self.bytes -= entry_bytes(&e.rendered);
            }
        }
        before - self.bytes
    }

    /// A rendering from the cache if still valid, else `make`'s (a fresh
    /// revision). `id`: the entry (a block's id or a piece's); `i`: the
    /// block it belongs to (its table values: definitions, footnotes,
    /// heading ids).
    fn cached(
        &mut self,
        doc: &mut MarkdownDoc,
        i: usize,
        id: u64,
        make: &mut MakeRendering<'_>,
    ) -> io::Result<(u64, u32, Arc<Rendered>)> {
        self.tick += 1;
        let b = doc.index().blocks()[i].clone();
        let provisional = self.provisional && doc.tables_pending();
        let tables_gen = if provisional {
            doc.current_generation()
        } else {
            doc.tables_generation()
        };
        let (highlight, upgrade) = self.highlighting();
        if let Some(e) = self.entries.get_mut(&id)
            && e.opts_gen == self.opts_gen
            && !(upgrade && highlight_ready(&e.rendered))
        {
            if e.tables_gen == tables_gen {
                e.tick = self.tick;
                return Ok((e.rev, e.lines, Arc::clone(&e.rendered)));
            }
            let sig = signature(doc.current_tables(), &b, &e.rendered);
            if sig == e.sig {
                e.tables_gen = tables_gen;
                e.tick = self.tick;
                return Ok((e.rev, e.lines, Arc::clone(&e.rendered)));
            }
        }
        let opts = if highlight { &self.opts_hl } else { &self.opts };
        let (rendered, lines) = make(doc, provisional, opts)?;
        let rendered = Arc::new(rendered);
        let sig = signature(doc.current_tables(), &b, &rendered);
        let rev = self.next_rev;
        self.next_rev += 1;
        self.bytes += entry_bytes(&rendered);
        if let Some(old) = self.entries.insert(
            id,
            Entry {
                rendered: Arc::clone(&rendered),
                sig,
                tables_gen,
                opts_gen: self.opts_gen,
                rev,
                lines,
                tick: self.tick,
            },
        ) {
            self.bytes -= entry_bytes(&old.rendered);
        }
        Ok((rev, lines, rendered))
    }

    /// The rendering of block `i`, from the cache if still valid.
    fn block<T: Text + ?Sized>(
        &mut self,
        doc: &mut MarkdownDoc,
        text: &T,
        i: usize,
    ) -> io::Result<(u64, u32, Arc<Rendered>)> {
        let b = doc.index().blocks()[i].clone();
        self.cached(doc, i, b.id, &mut |doc, provisional, opts| {
            let large = b.end - b.start > MAX_RENDER_BYTES;
            let bytes = if large {
                text.read(b.start..b.start + LARGE_PREFIX as u64 + 4)?
            } else {
                text.read(b.range())?
            };
            if large {
                // Not counted for huge blocks: an estimate from the bytes.
                let lines = ((b.end - b.start) / 40).min(u32::MAX as u64) as u32;
                return Ok((render_large(&bytes, b.end - b.start), lines));
            }
            let lines =
                (line_starts(&bytes).len() - usize::from(bytes.ends_with(b"\n"))).max(1) as u32;
            let r = if provisional {
                doc.render_bytes_provisional(&bytes, i, opts)
            } else {
                doc.render_bytes(&bytes, i, opts)
            };
            Ok((r, lines))
        })
    }

    /// Piece `k` of block `i`, shown as pieces with layout `l`.
    fn piece<T: Text + ?Sized>(
        &mut self,
        doc: &mut MarkdownDoc,
        text: &T,
        i: usize,
        l: &Layout,
        k: usize,
    ) -> io::Result<(u64, u32, Arc<Rendered>)> {
        let start = doc.index().blocks()[i].start;
        let p = &l.pieces[k];
        self.cached(doc, i, p.id, &mut |doc, provisional, opts| {
            let body = text.read(start + p.body.start as u64..start + p.body.end as u64)?;
            let src = l.source(&body);
            let mut r = if provisional {
                doc.render_bytes_provisional(&src, i, opts)
            } else {
                doc.render_bytes(&src, i, opts)
            };
            r.html = l.wrap(k, &r.html);
            Ok((r, p.lines))
        })
    }

    /// Block `i`'s piece layout, if it is shown as pieces.
    fn layout_of<T: Text + ?Sized>(
        &mut self,
        doc: &MarkdownDoc,
        text: &T,
        i: usize,
    ) -> io::Result<Option<Arc<Layout>>> {
        let b = &doc.index().blocks()[i];
        let candidate = matches!(
            b.kind,
            BlockKind::Table | BlockKind::CodeBlock { fenced: true, .. }
        );
        // Fewer bytes than the smallest pieced block could have lines.
        let size = b.end - b.start;
        if !candidate || size > MAX_RENDER_BYTES || size < 2 * pieces::TABLE_MIN_ROWS as u64 {
            return Ok(None);
        }
        if let Some(l) = self.layouts.get(&b.id) {
            return Ok(l.clone());
        }
        let bytes = text.read(b.range())?;
        let l = pieces::layout(&b.kind, &bytes).map(Arc::new);
        if self.layouts.len() >= 1024 {
            self.layouts.clear();
        }
        self.layouts.insert(b.id, l.clone());
        Ok(l)
    }

    /// The blocks around `offset`: `before` blocks before the one holding
    /// it and `after` after it, indexing as far as needed. `have(id, rev)`
    /// says whether the frontend already has a rendering; those go without
    /// HTML.
    ///
    /// A block shown as pieces contributes the pieces near the anchor: up to
    /// [`PIECES_AROUND`] on each side of the anchor's piece, or, when the
    /// anchor is outside it, those next to the anchor, and the window ends
    /// there (entries are contiguous; the frontend asks for more near its
    /// ends).
    pub fn window<T: Text + ?Sized>(
        &mut self,
        doc: &mut MarkdownDoc,
        text: &T,
        offset: u64,
        before: usize,
        after: usize,
        have: &dyn Fn(u64, u64) -> bool,
    ) -> io::Result<Window> {
        doc.extend_to(text, offset)?;
        let mut i = doc.index().block_at(offset);
        while doc.index().blocks().len() < i + after + 1 && !doc.index().is_complete() {
            doc.extend(text)?;
        }
        let n = doc.index().blocks().len();
        if n == 0 {
            return Ok(Window {
                frontier: doc.index().frontier(),
                complete: doc.index().is_complete(),
                len: text.len(),
                at_start: true,
                at_end: doc.index().is_complete(),
                ..Window::default()
            });
        }
        i = i.min(n - 1);
        let from = i.saturating_sub(before);
        let mut first = from;
        let last = (i + after).min(n - 1);
        let mut at_start = from == 0;
        let mut at_end = last == n - 1 && doc.index().is_complete();
        let mut blocks: Vec<WindowBlock> = Vec::with_capacity(last + 1 - from);
        let mut anchor = 0;
        let mut ids = std::collections::HashSet::new();
        for j in from..=last {
            let Some(l) = self.layout_of(doc, text, j)? else {
                let (rev, lines, r) = self.block(doc, text, j)?;
                let b = &doc.index().blocks()[j];
                if j == i {
                    anchor = blocks.len();
                }
                blocks.push(WindowBlock {
                    id: b.id,
                    start: b.start,
                    end: b.end,
                    kind: kind_code(&b.kind),
                    detail: kind_detail(&b.kind),
                    lines,
                    rev,
                    html: (!have(b.id, rev)).then_some(r),
                });
                continue;
            };
            let count = l.pieces.len();
            let b = doc.index().blocks()[j].clone();
            let at = (j == i).then(|| l.piece_at(offset.saturating_sub(b.start)));
            let (lo, hi) = match j.cmp(&i) {
                std::cmp::Ordering::Less => (count.saturating_sub(PIECES_AROUND), count),
                std::cmp::Ordering::Equal => {
                    let k = at.unwrap_or(0);
                    (
                        k.saturating_sub(PIECES_AROUND),
                        (k + PIECES_AROUND + 1).min(count),
                    )
                }
                std::cmp::Ordering::Greater => (0, PIECES_AROUND.min(count)),
            };
            if lo > 0 {
                // The window starts inside this block.
                blocks.clear();
                first = j;
                at_start = false;
            }
            for k in lo..hi {
                let (rev, lines, r) = self.piece(doc, text, j, &l, k)?;
                let p = &l.pieces[k];
                // Identical pieces of two blocks in one window: tell them
                // apart by their block (stable while it is unedited).
                let mut id = p.id;
                if !ids.insert(id) {
                    let mut h = Xxh3::new();
                    h.update(&id.to_le_bytes());
                    h.update(&b.id.to_le_bytes());
                    h.update(&(k as u64).to_le_bytes());
                    id = pieces::PIECE_ID_BIT | (h.digest() & (pieces::PIECE_ID_BIT - 1));
                    ids.insert(id);
                }
                if at == Some(k) {
                    anchor = blocks.len();
                }
                blocks.push(WindowBlock {
                    id,
                    start: b.start + p.start,
                    end: b.start + p.end,
                    kind: kind_code(&b.kind),
                    detail: kind_detail(&b.kind),
                    lines,
                    rev,
                    html: (!have(id, rev)).then_some(r),
                });
            }
            if hi < count {
                // The window ends inside this block.
                at_end = false;
                break;
            }
        }
        Ok(Window {
            first,
            anchor,
            blocks,
            at_start,
            at_end,
            indexed_blocks: n,
            frontier: doc.index().frontier(),
            complete: doc.index().is_complete(),
            len: text.len(),
        })
    }
}

/// Message magic: "MDW1".
pub const MAGIC: u32 = u32::from_le_bytes(*b"MDW1");

/// Encodes a window (little-endian; `apps/desktop/src/md/protocol.ts`):
///
/// ```text
/// u32 magic, u32 flags (1: index complete, 2: the window holds the
/// document's first entry, 4: its last), u64 seq, u64 revision,
/// u64 len, u64 frontier, u32 first, u32 anchor, u32 indexed blocks,
/// u32 count, then per block:
///   u64 id, u64 start, u64 end, u64 rev, u32 lines, u8 kind,
///   u8 has_html, u8 detail, u8 reserved, u32 html bytes, html (UTF-8)
/// ```
///
/// `seq` is the request's sequence number (0 for pushed updates) and
/// `revision` the text revision the window shows.
pub fn encode(w: &Window, seq: u64, revision: u64) -> Vec<u8> {
    let html: usize = w
        .blocks
        .iter()
        .filter_map(|b| b.html.as_ref())
        .map(|r| r.html.len())
        .sum();
    let mut out = Vec::with_capacity(56 + w.blocks.len() * 48 + html);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    let flags = u32::from(w.complete) | u32::from(w.at_start) << 1 | u32::from(w.at_end) << 2;
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&revision.to_le_bytes());
    out.extend_from_slice(&w.len.to_le_bytes());
    out.extend_from_slice(&w.frontier.to_le_bytes());
    out.extend_from_slice(&(w.first as u32).to_le_bytes());
    out.extend_from_slice(&(w.anchor as u32).to_le_bytes());
    out.extend_from_slice(&(w.indexed_blocks as u32).to_le_bytes());
    out.extend_from_slice(&(w.blocks.len() as u32).to_le_bytes());
    for b in &w.blocks {
        out.extend_from_slice(&b.id.to_le_bytes());
        out.extend_from_slice(&b.start.to_le_bytes());
        out.extend_from_slice(&b.end.to_le_bytes());
        out.extend_from_slice(&b.rev.to_le_bytes());
        out.extend_from_slice(&b.lines.to_le_bytes());
        out.push(b.kind);
        out.push(u8::from(b.html.is_some()));
        out.push(b.detail);
        out.push(0);
        let h = b.html.as_ref().map_or("", |r| r.html.as_str());
        out.extend_from_slice(&(h.len() as u32).to_le_bytes());
        out.extend_from_slice(h.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(text: &str) -> (MarkdownDoc, Preview) {
        (
            MarkdownDoc::new(text.len() as u64),
            Preview::new(RenderOptions::default()),
        )
    }

    fn html_of(w: &Window, id: u64) -> Option<String> {
        w.blocks
            .iter()
            .find(|b| b.id == id)
            .and_then(|b| b.html.as_ref().map(|r| r.html.clone()))
    }

    #[test]
    fn windows_send_html_only_for_what_the_frontend_lacks() {
        let text = (0..50).map(|i| format!("Para {i}\n\n")).collect::<String>();
        let (mut doc, mut p) = setup(&text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 2, 5, &|_, _| false)
            .unwrap();
        assert_eq!(w.first, 0);
        assert_eq!(w.anchor, 0);
        assert_eq!(w.blocks.len(), 6);
        assert!(w.blocks.iter().all(|b| b.html.is_some()));
        let have: Vec<(u64, u64)> = w.blocks.iter().map(|b| (b.id, b.rev)).collect();
        let mid = text.find("Para 20").unwrap() as u64 + 2;
        let w2 = p
            .window(&mut doc, text.as_bytes(), mid, 2, 2, &|id, rev| {
                have.contains(&(id, rev))
            })
            .unwrap();
        assert_eq!(w2.first, 18);
        assert_eq!(w2.anchor, 2);
        assert!(w2.blocks[2].html.as_ref().unwrap().html.contains("Para 20"));
        let again = p
            .window(&mut doc, text.as_bytes(), 0, 0, 3, &|id, rev| {
                have.contains(&(id, rev))
            })
            .unwrap();
        assert!(again.blocks.iter().all(|b| b.html.is_none()));
        let bytes = encode(&w2, 7, 3);
        assert_eq!(&bytes[..4], b"MDW1");
    }

    #[test]
    fn a_changed_definition_rerenders_only_its_users() {
        let mut text = String::from("See [x].\n\nPlain.\n\n# Title\n\n[x]: /one\n");
        let (mut doc, mut p) = setup(&text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 10, &|_, _| false)
            .unwrap();
        let ids: Vec<u64> = w.blocks.iter().map(|b| b.id).collect();
        let revs: Vec<u64> = w.blocks.iter().map(|b| b.rev).collect();
        assert!(html_of(&w, ids[0]).unwrap().contains("href=\"/one\""));
        // Edit the definition far away.
        let at = text.find("/one").unwrap() + 1;
        text.replace_range(at..at + 3, "two");
        doc.edit(text.as_bytes(), at as u64, 3, 3).unwrap();
        let w2 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 10, &|_, _| false)
            .unwrap();
        assert_eq!(w2.blocks[0].id, ids[0]);
        assert_ne!(w2.blocks[0].rev, revs[0], "the user re-renders");
        assert!(
            w2.blocks[0]
                .html
                .as_ref()
                .unwrap()
                .html
                .contains("href=\"/two\"")
        );
        assert_eq!(
            w2.blocks[1].rev, revs[1],
            "an unrelated block keeps its rendering"
        );
        // The definition's lines belong to the block before them (ADR-0027).
        assert_ne!(w2.blocks[2].id, ids[2], "the edited block is new");
    }

    #[test]
    fn new_options_rerender_and_trim_evicts() {
        let text = "![i](https://e.com/a.png)\n";
        let (mut doc, mut p) = setup(text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 0, &|_, _| false)
            .unwrap();
        assert!(
            html_of(&w, w.blocks[0].id)
                .unwrap()
                .contains("data-blocked-src")
        );
        let mut o = p.options().clone();
        if let Some(pol) = o.sanitize.as_mut() {
            pol.remote = Some(1);
        }
        p.set_options(o);
        let w2 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 0, &|_, _| true)
            .unwrap();
        assert!(w2.blocks[0].rev != w.blocks[0].rev);
        assert!(p.bytes() > 0);
        assert!(p.trim(0) > 0);
        assert_eq!(p.bytes(), 0);
        assert!(p.is_empty());
    }

    #[test]
    fn huge_blocks_show_their_start_labeled() {
        let mut text = String::from("```\n");
        while text.len() < (MAX_RENDER_BYTES as usize) + 10 {
            text.push_str("let x = 1; // <code>\n");
        }
        text.push_str("```\n\nafter\n");
        let (mut doc, mut p) = setup(&text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 1, &|_, _| false)
            .unwrap();
        let h = &w.blocks[0].html.as_ref().unwrap().html;
        assert!(h.starts_with("<div class=\"md-too-large\""));
        assert!(h.contains("&lt;code&gt;"));
        assert!(h.len() < 40_000);
        assert!(w.blocks[1].html.as_ref().unwrap().html.contains("after"));
    }
    #[test]
    fn large_tables_are_windowed_as_pieces_and_patched_by_piece() {
        let mut text = String::from("Intro.\n\n| A | B |\n|---|---|\n");
        for i in 0..1500 {
            text.push_str(&format!("| row {i} | {} |\n", i * 3));
        }
        text.push_str("\nAfter.\n");
        let (mut doc, mut p) = setup(&text);
        // From the start: the window ends inside the table.
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 10, &|_, _| false)
            .unwrap();
        assert!(w.at_start && !w.at_end);
        assert_eq!(w.blocks.len(), 1 + PIECES_AROUND);
        assert!(w.blocks[1].id & pieces::PIECE_ID_BIT != 0);
        let h1 = &w.blocks[1].html.as_ref().unwrap().html;
        assert!(
            h1.contains("md-piece-first") && h1.contains("<colgroup>"),
            "{}",
            &h1[..300.min(h1.len())]
        );
        for e in w.blocks.windows(2) {
            assert_eq!(e[0].end, e[1].start, "entries are contiguous");
        }
        // Inside the table: pieces around the anchor's.
        let at = text.find("| row 800 |").unwrap() as u64;
        let w = p
            .window(&mut doc, text.as_bytes(), at, 3, 3, &|_, _| false)
            .unwrap();
        assert!(!w.at_start && !w.at_end);
        assert_eq!(w.blocks.len(), 2 * PIECES_AROUND + 1);
        let a = &w.blocks[w.anchor];
        assert!(a.start <= at && at < a.end);
        assert!(a.html.as_ref().unwrap().html.contains("row 800"));
        // Edit a cell: only its piece comes again.
        let have: Vec<(u64, u64)> = w.blocks.iter().map(|b| (b.id, b.rev)).collect();
        let cell = at as usize + 2;
        text.replace_range(cell..cell + 1, "R");
        doc.edit(text.as_bytes(), cell as u64, 1, 1).unwrap();
        let w2 = p
            .window(&mut doc, text.as_bytes(), at, 3, 3, &|id, rev| {
                have.contains(&(id, rev))
            })
            .unwrap();
        let sent: Vec<&WindowBlock> = w2.blocks.iter().filter(|b| b.html.is_some()).collect();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].html.as_ref().unwrap().html.contains("Row 800"));
        // At the end: the window starts inside the table.
        let end = text.len() as u64 - 2;
        let w3 = p
            .window(&mut doc, text.as_bytes(), end, 5, 5, &|_, _| false)
            .unwrap();
        assert!(!w3.at_start && w3.at_end);
        assert_eq!(w3.blocks.len(), PIECES_AROUND + 1);
        assert!(
            w3.blocks[PIECES_AROUND - 1]
                .html
                .as_ref()
                .unwrap()
                .html
                .contains("md-piece-last")
        );
    }

    #[test]
    fn pieces_follow_their_block_after_an_edit_above_it() {
        let mut text = String::from("Intro.\n\nMore text after them.\n\n| A | B |\n|---|---|\n");
        for i in 0..300 {
            text.push_str(&format!("| row {i} | {i} |\n"));
        }
        let (mut doc, mut p) = setup(&text);
        let at = text.find("| row 150 |").unwrap() as u64;
        let w = p
            .window(&mut doc, text.as_bytes(), at, 3, 3, &|_, _| false)
            .unwrap();
        assert!(
            w.blocks[w.anchor]
                .html
                .as_ref()
                .unwrap()
                .html
                .contains("<table")
        );
        text.insert_str(0, "Hello ");
        doc.edit(text.as_bytes(), 0, 0, 6).unwrap();
        let at = at + 6;
        for have_all in [true, false] {
            let w = p
                .window(&mut doc, text.as_bytes(), at, 3, 3, &|_, _| have_all)
                .unwrap();
            let a = &w.blocks[w.anchor];
            assert!(a.start <= at && at < a.end, "{} {} {}", a.start, at, a.end);
            assert_eq!(&text[a.start as usize..a.start as usize + 6], "| row ");
            if !have_all {
                let h = &a.html.as_ref().unwrap().html;
                assert!(h.contains("<table") && h.contains("row 150"), "{h}");
            }
        }
    }

    #[test]
    fn provisional_windows_come_first_and_corrections_follow() {
        let mut text = String::from("Intro.\n\nLater[^n] text.\n\n[^n]: The note.\n");
        let (mut doc, mut p) = setup(&text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        assert!(w.blocks[1].html.as_ref().unwrap().html.contains(">1</a>"));
        // A new reference before it renumbers the note: rebuild pending.
        text.insert_str(0, "First[^n] ");
        doc.edit(text.as_bytes(), 0, 0, 10).unwrap();
        assert!(doc.tables_pending());
        p.set_provisional(true);
        let w1 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        assert!(
            doc.tables_pending(),
            "no rebuild for the provisional window"
        );
        assert!(w1.blocks[0].html.as_ref().unwrap().html.contains("First"));
        p.set_provisional(false);
        let w2 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        assert!(!doc.tables_pending());
        // The edited block gets its number; the later reference is the second.
        assert_ne!(w2.blocks[0].rev, w1.blocks[0].rev);
        assert!(w2.blocks[0].html.as_ref().unwrap().html.contains(">1</a>"));
        assert_ne!(w2.blocks[1].rev, w1.blocks[1].rev, "renumbered");
    }

    /// Waits until the background highlighter has what `w` waits for.
    fn wait_due(p: &Preview, w: &Window) {
        let shown: Vec<(u64, u64)> = w.blocks.iter().map(|b| (b.id, b.rev)).collect();
        let t0 = Instant::now();
        while !p.highlight_due(&shown) {
            assert!(t0.elapsed() < Duration::from_secs(20), "never highlighted");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn code_is_highlighted_after_the_window_and_when_edited() {
        let mut text = String::from(
            "Intro.

```rust
fn preview_a() {}
```

```cobol
X.
```
",
        );
        let (mut doc, mut p) = setup(&text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        let code = html_of(&w, w.blocks[1].id).unwrap();
        assert!(
            code.contains("data-lang=\"rust\" data-hl=\"pending\""),
            "{code}"
        );
        assert!(!code.contains("hl-keyword"));
        let other = html_of(&w, w.blocks[2].id).unwrap();
        assert!(other.contains("data-hl=\"none\""), "{other}");
        wait_due(&p, &w);

        // The pass renders the waiting block again, nothing else.
        p.set_highlight(Highlight::Upgrade(Duration::from_secs(10)));
        let w2 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        p.set_highlight(Highlight::Off);
        assert_eq!(w2.blocks[0].rev, w.blocks[0].rev);
        assert_ne!(w2.blocks[1].rev, w.blocks[1].rev);
        assert_eq!(
            w2.blocks[2].rev, w.blocks[2].rev,
            "no grammar: nothing to do"
        );
        let code = html_of(&w2, w2.blocks[1].id).unwrap();
        assert!(
            code.contains("hl-keyword") && !code.contains("data-hl"),
            "{code}"
        );
        let shown: Vec<(u64, u64)> = w2.blocks.iter().map(|b| (b.id, b.rev)).collect();
        assert!(!p.highlight_due(&shown));

        // An edited block is highlighted in its first window (its grammar
        // is ready now).
        let at = text.find("{}").unwrap();
        text.insert_str(at, "1 ");
        doc.edit(text.as_bytes(), at as u64, 0, 2).unwrap();
        p.set_highlight(Highlight::New);
        let w3 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        p.set_highlight(Highlight::Off);
        let code = html_of(&w3, w3.blocks[1].id).unwrap();
        assert!(
            code.contains("hl-keyword") && code.contains(">1</span> "),
            "{code}"
        );
    }

    #[test]
    fn an_expired_pass_leaves_code_pending() {
        let text = "```rust
fn preview_expired() {}
```
";
        let (mut doc, mut p) = setup(text);
        let w = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        wait_due(&p, &w);
        p.set_highlight(Highlight::Upgrade(Duration::ZERO));
        let w2 = p
            .window(&mut doc, text.as_bytes(), 0, 0, 5, &|_, _| false)
            .unwrap();
        assert_eq!(w2.blocks[0].rev, w.blocks[0].rev);
    }
}
