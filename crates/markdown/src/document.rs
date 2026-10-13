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
    tables: Tables,
    tables_dirty: bool,
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
            tables: Tables::default(),
            tables_dirty: true,
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
        let new = self.index.extend(text)?;
        self.analyze_range(text, new)
    }

    fn analyze_range<T: Text + ?Sized>(
        &mut self,
        text: &T,
        range: std::ops::Range<usize>,
    ) -> io::Result<()> {
        for i in range {
            let b = &self.index.blocks()[i];
            let (id, r) = (b.id, b.range());
            if self.analyzed.contains(&id) {
                continue;
            }
            let a = analyze(&text.read(r)?, self.index.flavor());
            self.analyzed.insert(id);
            if !a.is_empty() {
                self.analyses.insert(id, Arc::new(a));
                self.tables_dirty = true;
            }
        }
        Ok(())
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
        for id in &ch.removed {
            self.analyzed.remove(id);
            if self.analyses.remove(id).is_some() {
                self.tables_dirty = true;
            }
        }
        self.analyze_range(text, ch.first..ch.first + ch.inserted)?;
        Ok(ch)
    }

    /// The tables, rebuilt if a block that takes part in them changed.
    pub fn tables(&mut self) -> &Tables {
        if self.tables_dirty {
            let complete = self.index.is_complete();
            let analyses = &self.analyses;
            self.tables = Tables::build(
                self.index.blocks(),
                |id| analyses.get(&id).map(|a| a.as_ref()),
                complete,
            );
            self.tables_dirty = false;
        }
        &self.tables
    }

    /// Renders block `i` of the index.
    pub fn render<T: Text + ?Sized>(
        &mut self,
        text: &T,
        i: usize,
        opts: &RenderOptions,
    ) -> io::Result<Rendered> {
        let b = self.index.blocks()[i].clone();
        let bytes = text.read(b.range())?;
        if b.kind == BlockKind::FrontMatter {
            return Ok(render_front_matter(&bytes, opts));
        }
        let tables = self.tables();
        Ok(render_block(&bytes, b.id, tables, opts))
    }
}

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
