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

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use xxhash_rust::xxh3::Xxh3;

use crate::analysis::normalize_label;
use crate::document::MarkdownDoc;
use crate::index::Block;
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
    /// Index of the first block in the document.
    pub first: usize,
    /// Position of the block holding the requested offset within `blocks`.
    pub anchor: usize,
    pub blocks: Vec<WindowBlock>,
    /// Blocks indexed so far, and where the index ends.
    pub indexed_blocks: usize,
    pub frontier: u64,
    pub complete: bool,
    pub len: u64,
}

/// The preview's render cache.
pub struct Preview {
    entries: HashMap<u64, Entry>,
    /// Render with the tables as they are, without waiting for a rebuild.
    provisional: bool,
    opts: RenderOptions,
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
        Self {
            entries: HashMap::new(),
            provisional: false,
            opts,
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
        self.opts = opts;
        self.opts_gen += 1;
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

    /// The rendering of block `i`, from the cache if still valid.
    fn block<T: Text + ?Sized>(
        &mut self,
        doc: &mut MarkdownDoc,
        text: &T,
        i: usize,
    ) -> io::Result<(u64, u32, Arc<Rendered>)> {
        self.tick += 1;
        let b = doc.index().blocks()[i].clone();
        let provisional = self.provisional && doc.tables_pending();
        let tables_gen = if provisional {
            doc.current_generation()
        } else {
            doc.tables_generation()
        };
        if let Some(e) = self.entries.get_mut(&b.id)
            && e.opts_gen == self.opts_gen
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
        let large = b.end - b.start > MAX_RENDER_BYTES;
        let bytes = if large {
            text.read(b.start..b.start + LARGE_PREFIX as u64 + 4)?
        } else {
            text.read(b.range())?
        };
        let lines = if large {
            // Not counted for huge blocks: an estimate from the bytes.
            ((b.end - b.start) / 40).min(u32::MAX as u64) as u32
        } else {
            (line_starts(&bytes).len() - usize::from(bytes.ends_with(b"\n"))).max(1) as u32
        };
        let rendered = Arc::new(if large {
            render_large(&bytes, b.end - b.start)
        } else {
            if provisional {
                doc.render_bytes_provisional(&bytes, i, &self.opts)
            } else {
                doc.render_bytes(&bytes, i, &self.opts)
            }
        });
        let sig = signature(doc.current_tables(), &b, &rendered);
        let rev = self.next_rev;
        self.next_rev += 1;
        self.bytes += entry_bytes(&rendered);
        if let Some(old) = self.entries.insert(
            b.id,
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

    /// The blocks around `offset`: `before` blocks before the one holding
    /// it and `after` after it, indexing as far as needed. `have(id, rev)`
    /// says whether the frontend already has a rendering; those go without
    /// HTML.
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
                ..Window::default()
            });
        }
        i = i.min(n - 1);
        let first = i.saturating_sub(before);
        let last = (i + after).min(n - 1);
        let mut blocks = Vec::with_capacity(last + 1 - first);
        for j in first..=last {
            let (rev, lines, r) = self.block(doc, text, j)?;
            let b = &doc.index().blocks()[j];
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
        }
        Ok(Window {
            first,
            anchor: i - first,
            blocks,
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
/// u32 magic, u32 flags (1: index complete), u64 seq, u64 revision,
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
    out.extend_from_slice(&u32::from(w.complete).to_le_bytes());
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
}
