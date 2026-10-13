//! A Markdown document's derived state: the block index, the blocks'
//! analyses and the document-wide tables, kept in step with the text.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use crate::analysis::{Analysis, analyze};
use crate::index::{BlockIndex, CHUNK, Change, EDIT_BUDGET};
use crate::options::Flavor;
use crate::render::{RenderOptions, Rendered, render_block};
use crate::scan::BlockKind;
use crate::tables::Tables;
use crate::text::Text;

#[derive(Debug)]
pub struct MarkdownDoc {
    index: BlockIndex,
    /// Analyses of blocks that take part in a table (absent: none, or not
    /// analyzed yet; `analyzed` tells which).
    analyses: HashMap<u64, Arc<Analysis>>,
    analyzed: std::collections::HashSet<u64>,
    /// The analyses again, one per indexed block in order (rebuilding the
    /// tables walks this, not 200,000 map lookups).
    aligned: Vec<Option<Arc<Analysis>>>,
    tables: Tables,
    tables_dirty: bool,
    /// Only the outline is stale (heading text changed, nothing else).
    outline_dirty: bool,
    /// Bumped each time the tables are rebuilt.
    tables_gen: u64,
}

impl MarkdownDoc {
    pub fn new(len: u64) -> Self {
        Self::with_flavor(len, Flavor::PRODUCT)
    }

    pub fn with_flavor(len: u64, flavor: Flavor) -> Self {
        Self {
            index: BlockIndex::with_flavor(len, CHUNK, flavor),
            analyses: HashMap::new(),
            analyzed: Default::default(),
            aligned: Vec::new(),
            tables: Tables::default(),
            tables_dirty: true,
            outline_dirty: false,
            tables_gen: 0,
        }
    }

    pub fn index(&self) -> &BlockIndex {
        &self.index
    }

    /// Indexes and analyzes up to `offset` (at least one stretch).
    pub fn extend_to<T: Text + ?Sized>(&mut self, text: &T, offset: u64) -> io::Result<()> {
        let first = self.index.blocks().len();
        self.index.extend_to(text, offset)?;
        self.analyze_range(text, first..self.index.blocks().len())
    }

    /// Indexes and analyzes one more stretch.
    pub fn extend<T: Text + ?Sized>(&mut self, text: &T) -> io::Result<()> {
        let was_complete = self.index.is_complete();
        let new = self.index.extend(text)?;
        if self.index.is_complete() != was_complete {
            // Footnote numbers wait for the whole document.
            self.tables_dirty = true;
        }
        self.analyze_range(text, new)
    }

    fn analyze_range<T: Text + ?Sized>(
        &mut self,
        text: &T,
        range: std::ops::Range<usize>,
    ) -> io::Result<()> {
        for i in range {
            if self.analyze_block(text, i)? {
                self.tables_dirty = true;
            }
        }
        self.sync_aligned();
        Ok(())
    }

    /// Appends the analyses of blocks the index gained at its end.
    fn sync_aligned(&mut self) {
        let blocks = self.index.blocks();
        for b in &blocks[self.aligned.len().min(blocks.len())..] {
            self.aligned.push(self.analyses.get(&b.id).cloned());
        }
        debug_assert_eq!(self.aligned.len(), self.index.blocks().len());
    }

    /// The analyzed blocks in order, for building the tables.
    fn analyzed_blocks(&self) -> impl Iterator<Item = (u64, &Analysis)> {
        self.index
            .blocks()
            .iter()
            .zip(&self.aligned)
            .filter_map(|(b, a)| a.as_deref().map(|a| (b.id, a)))
    }

    /// Analyzes block `i` unless it was; returns whether it takes part in
    /// the tables. Front matter is YAML, not Markdown: it never does.
    fn analyze_block<T: Text + ?Sized>(&mut self, text: &T, i: usize) -> io::Result<bool> {
        let b = &self.index.blocks()[i];
        let (id, r) = (b.id, b.range());
        if self.analyzed.contains(&id) {
            return Ok(false);
        }
        self.analyzed.insert(id);
        if b.kind == BlockKind::FrontMatter {
            return Ok(false);
        }
        let a = analyze(&text.read(r)?, self.index.flavor());
        if a.is_empty() {
            return Ok(false);
        }
        self.analyses.insert(id, Arc::new(a));
        Ok(true)
    }

    /// Applies an edit to the derived state (`text` is after the edit).
    pub fn edit<T: Text + ?Sized>(
        &mut self,
        text: &T,
        from: u64,
        removed: u64,
        inserted: u64,
    ) -> io::Result<Change> {
        let ch = self
            .index
            .edit(text, from, removed, inserted, EDIT_BUDGET)?;
        let old: Vec<Option<Arc<Analysis>>> = ch
            .replaced
            .iter()
            .map(|id| self.analyses.get(id).cloned())
            .collect();
        let was_dirty = self.tables_dirty;
        for id in &ch.removed {
            self.analyzed.remove(id);
            self.analyses.remove(id);
        }
        // Analyze the new blocks without marking the tables (decided below).
        for i in ch.first..ch.first + ch.inserted {
            self.analyze_block(text, i)?;
        }
        let fresh: Vec<Option<Arc<Analysis>>> = self.index.blocks()
            [ch.first..ch.first + ch.inserted]
            .iter()
            .map(|b| self.analyses.get(&b.id).cloned())
            .collect();
        let end = (ch.first + ch.replaced.len()).min(self.aligned.len());
        self.aligned.splice(ch.first.min(end)..end, fresh);
        debug_assert_eq!(self.aligned.len(), self.index.blocks().len());
        self.tables_dirty = was_dirty;
        if !was_dirty {
            self.update_tables(&ch, &old);
        }
        Ok(ch)
    }

    /// Keeps the tables in step with an edit as cheaply as it can: block
    /// contributions that didn't change only rename ids; changed headings
    /// alone rebuild the outline; anything else rebuilds the tables.
    fn update_tables(&mut self, ch: &Change, old: &[Option<Arc<Analysis>>]) {
        let new_ids: Vec<u64> = self.index.blocks()[ch.first..ch.first + ch.inserted]
            .iter()
            .map(|b| b.id)
            .collect();
        let empty = Analysis::default();
        let new: Vec<Option<Arc<Analysis>>> = new_ids
            .iter()
            .map(|id| self.analyses.get(id).cloned())
            .collect();
        if old.len() != new.len() {
            // Blocks split or merged: unless none of them takes part.
            let any = old.iter().chain(new.iter()).any(Option::is_some);
            self.tables_dirty |= any;
            return;
        }
        // An id reused at another position (blocks shifted within the
        // edit): renaming in place could collide; rebuild instead.
        let shifted = new_ids
            .iter()
            .zip(&ch.replaced)
            .any(|(n, o)| n != o && ch.replaced.contains(n));
        if shifted {
            self.tables_dirty = true;
            return;
        }
        let mut headings_only = false;
        for (o, n) in old.iter().zip(&new) {
            let (oa, na) = (
                o.as_deref().unwrap_or(&empty),
                n.as_deref().unwrap_or(&empty),
            );
            if oa == na {
                continue;
            }
            if oa.defs == na.defs
                && oa.footnote_refs == na.footnote_refs
                && oa.footnote_defs == na.footnote_defs
            {
                headings_only = true;
            } else {
                self.tables_dirty = true;
                return;
            }
        }
        for ((old_id, o), new_id) in ch.replaced.iter().zip(old).zip(&new_ids) {
            if let Some(a) = o
                && old_id != new_id
            {
                self.tables.rename(*old_id, *new_id, a);
            }
        }
        if headings_only {
            self.outline_dirty = true;
        }
    }

    /// The tables, rebuilt if a block that takes part in them changed.
    pub fn tables(&mut self) -> &Tables {
        if self.outline_dirty && !self.tables_dirty {
            let mut tables = std::mem::take(&mut self.tables);
            tables.rebuild_outline(self.analyzed_blocks());
            self.tables = tables;
            self.outline_dirty = false;
            self.tables_gen += 1;
        }
        if self.tables_dirty {
            self.outline_dirty = false;
            let complete = self.index.is_complete();
            self.tables = Tables::build(self.analyzed_blocks(), complete);
            self.tables_dirty = false;
            self.tables_gen += 1;
        }
        &self.tables
    }

    /// Whether the tables wait for a rebuild (an edit changed what a block
    /// contributes): [`tables`](Self::tables) will rebuild them.
    pub fn tables_pending(&self) -> bool {
        self.tables_dirty || self.outline_dirty
    }

    /// The tables as they are, without rebuilding them (a provisional
    /// rendering right after an edit; see `Preview::set_provisional`).
    pub fn current_tables(&self) -> &Tables {
        &self.tables
    }

    /// The generation of [`current_tables`](Self::current_tables).
    pub fn current_generation(&self) -> u64 {
        self.tables_gen
    }

    /// Renders block `i` with the tables as they are (no rebuild).
    pub fn render_bytes_provisional(
        &self,
        bytes: &[u8],
        i: usize,
        opts: &RenderOptions,
    ) -> Rendered {
        let b = &self.index.blocks()[i];
        if b.kind == BlockKind::FrontMatter {
            return render_front_matter(bytes, opts);
        }
        render_block(bytes, b.id, &self.tables, opts)
    }

    /// Generation of the tables: changes when they are rebuilt (call
    /// [`tables`](Self::tables) first to rebuild them if needed).
    pub fn tables_generation(&mut self) -> u64 {
        self.tables();
        self.tables_gen
    }

    /// Renders block `i`, whose bytes are `bytes`.
    pub fn render_bytes(&mut self, bytes: &[u8], i: usize, opts: &RenderOptions) -> Rendered {
        let b = &self.index.blocks()[i];
        if b.kind == BlockKind::FrontMatter {
            return render_front_matter(bytes, opts);
        }
        let id = b.id;
        render_block(bytes, id, self.tables(), opts)
    }

    /// Renders block `i` of the index.
    pub fn render<T: Text + ?Sized>(
        &mut self,
        text: &T,
        i: usize,
        opts: &RenderOptions,
    ) -> io::Result<Rendered> {
        let bytes = text.read(self.index.blocks()[i].range())?;
        Ok(self.render_bytes(&bytes, i, opts))
    }
}

#[cfg(test)]
mod tests;

/// Front matter as a collapsed metadata panel (YAML shown as text).
fn render_front_matter(bytes: &[u8], opts: &RenderOptions) -> Rendered {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let body = if lines.len() >= 2 {
        lines[1..lines.len() - 1].join("\n")
    } else {
        String::new()
    };
    let mut html = String::from("<details class=\"front-matter\"");
    if opts.sourcepos {
        html.push_str(&format!(" data-sourcepos=\"1:1-{}:3\"", lines.len().max(1)));
    }
    html.push_str("><summary>Front matter</summary><pre><code class=\"language-yaml\">");
    let mut esc = String::new();
    let _ = comrak::html::escape(&mut esc, &body);
    html.push_str(&esc);
    if !body.is_empty() {
        html.push('\n');
    }
    html.push_str("</code></pre></details>\n");
    Rendered {
        html,
        ..Rendered::default()
    }
}

/// Renders a whole text through the block pipeline (index, analyses,
/// tables, one block at a time): what the spec harness and exports use.
/// Footnote definitions become GitHub's footnotes section at the end.
pub fn render_document(bytes: &[u8], opts: &RenderOptions) -> String {
    let opts = RenderOptions {
        document: true,
        ..opts.clone()
    };
    let mut doc = MarkdownDoc::with_flavor(bytes.len() as u64, opts.flavor);
    // Reading a byte slice can't fail.
    let _ = doc.extend_to(bytes, bytes.len() as u64);
    let mut html = String::new();
    let mut notes: Vec<(u32, String)> = Vec::new();
    for i in 0..doc.index().blocks().len() {
        let kind = doc.index().blocks()[i].kind.clone();
        let Ok(r) = doc.render(bytes, i, &opts) else {
            continue;
        };
        if let BlockKind::FootnoteDefinition { label } = kind {
            let key = crate::analysis::normalize_label(&label);
            if let Some(n) = doc.tables().footnote_number(&key) {
                notes.push((n, r.html));
            }
            continue;
        }
        html.push_str(&r.html);
    }
    if !notes.is_empty() {
        notes.sort_by_key(|n| n.0);
        html.push_str("<section class=\"footnotes\" data-footnotes>\n<ol>\n");
        for (_, item) in notes {
            html.push_str(&item);
        }
        html.push_str("</ol>\n</section>\n");
    }
    html
}
