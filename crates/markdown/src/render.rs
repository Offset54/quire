//! Block rendering: comrak's HTML formatter with our overrides (ADR-0026).
//!
//! A block is parsed on its own. References to definitions in other blocks
//! resolve through comrak's broken-link callback from the document-wide
//! [`Tables`], and every label the callback is asked about is recorded: the
//! block depends on it, and is re-rendered when its definition changes.
//! Footnote references parse only next to a definition, so stub definitions
//! are appended for the footnotes the block mentions that are defined
//! elsewhere; they are not rendered. Footnote numbers and reference ids come
//! from the tables, so they are the same whichever block they are in.

use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex};

use comrak::html::{ChildRendering, Context, format_document_with_formatter, format_node_default};
use comrak::nodes::{AstNode, NodeValue};
use comrak::options::{BrokenLinkReference, Plugins};
use comrak::{Arena, ResolvedReference, parse_document};

use crate::analysis::{footnote_candidates, normalize_label};
use crate::images::{ImagePolicy, ImageSrc};
use crate::options::{self, Flavor};
use crate::sanitize::sanitize;
use crate::tables::Tables;
use crate::text::same_len_utf8;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// `data-sourcepos` on elements (lines relative to the block, from 1).
    pub sourcepos: bool,
    /// `id` on headings, from the outline (GitHub's slugs).
    pub heading_ids: bool,
    pub flavor: Flavor,
    /// Whole-document output (exports): footnote definitions become items of
    /// the footnotes section at the end, as GitHub renders them. Otherwise
    /// (the preview) they stay where they are written.
    pub document: bool,
    /// Raw HTML through the allowlist sanitizer and image sources through
    /// this policy (ADR-0028). `None` only for the spec harness, which
    /// compares the renderer's output before sanitization.
    pub sanitize: Option<ImagePolicy>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            sourcepos: true,
            heading_ids: true,
            flavor: Flavor::PRODUCT,
            document: false,
            sanitize: Some(ImagePolicy::default()),
        }
    }
}

/// A rendered block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub html: String,
    /// Normalized labels of references resolved (or not) from other blocks.
    pub deps: Vec<String>,
    /// Footnote labels the block references.
    pub footnotes: Vec<String>,
}

struct State<'r> {
    tables: &'r Tables,
    block: u64,
    /// Lines of the block itself; stub definitions come after.
    base_lines: usize,
    opts: &'r RenderOptions,
    headings: usize,
    /// An element dropped with its content (`<script>`, …) is open: raw HTML
    /// and text are left out until it closes (one state per block).
    skip: Option<String>,
}

fn is_stub<'a>(node: &'a AstNode<'a>, base: usize) -> bool {
    node.data().sourcepos.start.line > base
}

/// URLs that never become links or images (comrak's own list:
/// `javascript:`, `vbscript:`, `file:` and `data:` other than images).
fn dangerous(url: &str) -> bool {
    comrak::html::dangerous_url(url)
}

fn sourcepos<'a, T>(context: &mut Context<T>, node: &'a AstNode<'a>) -> fmt::Result {
    comrak::html::render_sourcepos(context, node)
}

/// Tags GFM's tag filter disables.
const FILTERED_TAGS: &[&str] = &[
    "title",
    "textarea",
    "style",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "script",
    "plaintext",
];

/// GFM's tag filter: the `<` of a filtered tag (opening or closing) becomes
/// `&lt;`, so the tag shows as text.
pub fn tagfilter(html: &str) -> String {
    let b = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for (i, _) in html.match_indices('<') {
        let mut j = i + 1;
        if b.get(j) == Some(&b'/') {
            j += 1;
        }
        let filtered = FILTERED_TAGS.iter().any(|t| {
            let end = j + t.len();
            end <= b.len()
                && b[j..end].eq_ignore_ascii_case(t.as_bytes())
                && matches!(
                    b.get(end),
                    None | Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'>' | b'/')
                )
        });
        if filtered {
            out.push_str(&html[last..i]);
            out.push_str("&lt;");
            last = i + 1;
        }
    }
    out.push_str(&html[last..]);
    out
}

/// GitHub's back references at the end of a footnote: one per reference.
fn backrefs(context: &mut Context<State<'_>>, name: &str, key: &str) -> fmt::Result {
    let tables = context.user.tables;
    let number = tables.footnote_number(key).unwrap_or(0);
    let total = tables.footnote_refs_total(key).max(1);
    for k in 1..=total {
        if k > 1 {
            context.write_str(" ")?;
        }
        context.write_str("<a href=\"#fnref-")?;
        context.escape_href(name)?;
        if k > 1 {
            write!(context, "-{k}")?;
        }
        let idx = if k == 1 {
            number.to_string()
        } else {
            format!("{number}-{k}")
        };
        write!(
            context,
            "\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"{idx}\" aria-label=\"Back to reference {idx}\">↩"
        )?;
        if k > 1 {
            write!(context, "<sup class=\"footnote-ref\">{k}</sup>")?;
        }
        context.write_str("</a>")?;
    }
    Ok(())
}

/// The footnote definition a paragraph ends, if it is that definition's
/// last child (its back references go inside the paragraph).
fn closing_footnote<'a>(node: &'a AstNode<'a>, base: usize) -> Option<String> {
    let parent = node.parent()?;
    if node.next_sibling().is_some() || is_stub(parent, base) {
        return None;
    }
    match &parent.data().value {
        NodeValue::FootnoteDefinition(f) => Some(f.name.clone()),
        _ => None,
    }
}

fn format<'a>(
    context: &mut Context<State<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    let value = node.data().value.clone();
    match value {
        NodeValue::Heading(h) if context.user.opts.heading_ids => {
            if entering {
                context.cr()?;
                write!(context, "<h{}", h.level)?;
                let index = context.user.headings;
                context.user.headings += 1;
                if let Some(slug) = context.user.tables.heading_slug(context.user.block, index) {
                    let slug = slug.to_owned();
                    context.write_str(" id=\"")?;
                    context.escape(&slug)?;
                    context.write_str("\"")?;
                }
                sourcepos(context, node)?;
                context.write_str(">")?;
            } else {
                write!(context, "</h{}>", h.level)?;
                context.lf()?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::Link(l) => {
            if entering {
                context.write_str("<a")?;
                sourcepos(context, node)?;
                context.write_str(" href=\"")?;
                if !dangerous(&l.url) {
                    context.escape_href(&l.url)?;
                }
                if !l.title.is_empty() {
                    context.write_str("\" title=\"")?;
                    context.escape(&l.title)?;
                }
                context.write_str("\">")?;
            } else {
                context.write_str("</a>")?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::Image(l) => {
            if entering {
                context.write_str("<img")?;
                sourcepos(context, node)?;
                match &context.user.opts.sanitize {
                    Some(policy) => match policy.resolve(&l.url) {
                        ImageSrc::Url(u) => {
                            context.write_str(" src=\"")?;
                            context.escape_href(&u)?;
                        }
                        ImageSrc::Lazy(u) => {
                            context.write_str(" data-src=\"")?;
                            context.escape(&u)?;
                        }
                        ImageSrc::Placeholder(u) => {
                            context.write_str(" data-blocked-src=\"")?;
                            context.escape(&u)?;
                        }
                    },
                    None => {
                        context.write_str(" src=\"")?;
                        if !dangerous(&l.url) {
                            context.escape_href(&l.url)?;
                        }
                    }
                }
                context.write_str("\" alt=\"")?;
                return Ok(ChildRendering::Plain);
            }
            if !l.title.is_empty() {
                context.write_str("\" title=\"")?;
                context.escape(&l.title)?;
            }
            context.write_str("\" />")?;
            Ok(ChildRendering::HTML)
        }
        NodeValue::HtmlBlock(ref h) if context.user.opts.sanitize.is_some() => {
            if entering {
                context.cr()?;
                let clean = clean_html(context, &h.literal);
                context.write_str(&clean)?;
                context.cr()?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::HtmlInline(ref s) if context.user.opts.sanitize.is_some() => {
            if entering {
                let clean = clean_html(context, s);
                context.write_str(&clean)?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::Text(_) | NodeValue::Code(_) | NodeValue::SoftBreak | NodeValue::LineBreak
            if context.user.skip.is_some() =>
        {
            Ok(ChildRendering::Skip)
        }
        NodeValue::HtmlBlock(ref h) if context.user.opts.flavor.tagfilter => {
            if entering {
                context.cr()?;
                let filtered = tagfilter(&h.literal);
                context.write_str(&filtered)?;
                context.cr()?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::HtmlInline(ref s) if context.user.opts.flavor.tagfilter => {
            if entering {
                let filtered = tagfilter(s);
                context.write_str(&filtered)?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::FootnoteReference(r) => {
            if entering {
                let key = normalize_label(&r.name);
                let tables = context.user.tables;
                let number = tables.footnote_number(&key);
                let nth = tables.footnote_refs_before(context.user.block, &key) + r.ref_num;
                let mut id = format!("fnref-{}", r.name);
                if nth > 1 {
                    let _ = write!(id, "-{nth}");
                }
                context.write_str("<sup")?;
                sourcepos(context, node)?;
                context.write_str(" class=\"footnote-ref\"><a href=\"#fn-")?;
                context.escape_href(&r.name)?;
                context.write_str("\" id=\"")?;
                context.escape_href(&id)?;
                context.write_str("\" data-footnote-ref>")?;
                match number {
                    Some(n) => write!(context, "{n}")?,
                    // Numbers come from the whole document; until it is
                    // indexed, show the label (§7: never silently wrong).
                    None => context.escape(&r.name)?,
                }
                context.write_str("</a></sup>")?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::Paragraph if !entering && context.user.opts.document => {
            if let Some(name) = closing_footnote(node, context.user.base_lines) {
                context.write_str(" ")?;
                backrefs(context, &name, &normalize_label(&name))?;
            }
            format_node_default(context, node, entering)
        }
        NodeValue::FootnoteDefinition(f) => {
            if is_stub(node, context.user.base_lines) {
                return Ok(ChildRendering::Skip);
            }
            let key = normalize_label(&f.name);
            let number = context.user.tables.footnote_number(&key);
            if context.user.opts.document {
                // An item of the footnotes section; unreferenced definitions
                // are left out, as GitHub does.
                if number.is_none() {
                    return Ok(ChildRendering::Skip);
                }
                if entering {
                    context.cr()?;
                    context.write_str("<li")?;
                    sourcepos(context, node)?;
                    context.write_str(" id=\"fn-")?;
                    context.escape_href(&f.name)?;
                    context.write_str("\">")?;
                    context.lf()?;
                } else {
                    let last_is_paragraph = node
                        .last_child()
                        .is_some_and(|c| matches!(c.data().value, NodeValue::Paragraph));
                    if !last_is_paragraph {
                        backrefs(context, &f.name, &key)?;
                        context.lf()?;
                    }
                    context.write_str("</li>")?;
                    context.lf()?;
                }
                return Ok(ChildRendering::HTML);
            }
            if entering {
                context.cr()?;
                context.write_str("<div class=\"footnote-def\"")?;
                sourcepos(context, node)?;
                context.write_str(" id=\"fn-")?;
                context.escape_href(&f.name)?;
                context.write_str("\"><span class=\"footnote-num\">")?;
                match number {
                    Some(n) => write!(context, "{n}")?,
                    None => context.escape(&f.name)?,
                }
                context.write_str("</span>")?;
                context.lf()?;
            } else {
                context.write_str("<a href=\"#fnref-")?;
                context.escape_href(&f.name)?;
                context.write_str(
                    "\" class=\"footnote-backref\" aria-label=\"Back to reference\">↩</a></div>",
                )?;
                context.lf()?;
            }
            Ok(ChildRendering::HTML)
        }
        _ => format_node_default(context, node, entering),
    }
}

/// Raw HTML through the sanitizer, image sources through the policy.
fn clean_html(context: &mut Context<State<'_>>, html: &str) -> String {
    let state = &mut context.user;
    let policy = state.opts.sanitize.as_ref();
    let mut image = |src: &str| match policy {
        Some(p) => p.resolve(src),
        None => ImageSrc::Placeholder(src.to_owned()),
    };
    sanitize(html, &mut image, &mut state.skip)
}

/// Renders one block's bytes. `block` is its id (heading ids, footnote
/// reference ids).
pub fn render_block(bytes: &[u8], block: u64, tables: &Tables, opts: &RenderOptions) -> Rendered {
    let text = same_len_utf8(bytes);
    let mut source = text.into_owned();
    if !source.ends_with('\n') {
        source.push('\n');
    }
    let base_lines = source.matches('\n').count();
    let mut footnotes = Vec::new();
    let candidates = if opts.flavor.footnotes {
        footnote_candidates(&source)
    } else {
        Vec::new()
    };
    for label in candidates {
        let key = normalize_label(&label);
        if tables.footnote_defined(&key) {
            footnotes.push(key);
            source.push_str("\n[^");
            source.push_str(&label);
            source.push_str("]: \u{200b}\n");
        }
    }

    let deps = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut o = options::block(&opts.flavor);
    o.render.sourcepos = opts.sourcepos;
    o.parse.broken_link_callback = Some(Arc::new(Resolver {
        tables,
        deps: Arc::clone(&deps),
    }));

    let arena = Arena::new();
    let root = parse_document(&arena, &source, &o);
    let mut html = String::new();
    let state = State {
        tables,
        block,
        base_lines,
        opts,
        headings: 0,
        skip: None,
    };
    let _ = format_document_with_formatter(root, &o, &mut html, &Plugins::default(), format, state);
    let deps = std::mem::take(&mut *deps.lock().unwrap_or_else(|e| e.into_inner()));
    Rendered {
        html,
        deps,
        footnotes,
    }
}

/// Answers comrak's broken-link callback from the document's tables and
/// records the labels asked about.
struct Resolver<'r> {
    tables: &'r Tables,
    deps: Arc<Mutex<Vec<String>>>,
}

impl comrak::options::BrokenLinkCallback for Resolver<'_> {
    fn resolve(&self, r: BrokenLinkReference<'_>) -> Option<ResolvedReference> {
        let key = normalize_label(r.original);
        let found = self.tables.definition(&key).map(|d| ResolvedReference {
            url: d.url.clone(),
            title: d.title.clone(),
        });
        let mut deps = self.deps.lock().unwrap_or_else(|e| e.into_inner());
        if !deps.contains(&key) {
            deps.push(key);
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagfilter_shows_filtered_tags_as_text() {
        assert_eq!(
            tagfilter("<strong> <title> <style> <em></STYLE> <scripts> <xmp/>"),
            "<strong> &lt;title> &lt;style> <em>&lt;/STYLE> <scripts> &lt;xmp/>"
        );
    }
}
