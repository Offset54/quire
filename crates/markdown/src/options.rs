//! Which Markdown is parsed: the extensions §3.1 lists (ADR-0026), all on in
//! the product. The spec harness turns them on per suite.

use comrak::Options;

/// The extension set. Block boundaries depend on it (a table is a block only
/// with tables on), so the index, analysis and rendering share one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flavor {
    pub table: bool,
    pub strikethrough: bool,
    pub autolink: bool,
    pub tasklist: bool,
    pub footnotes: bool,
    /// `$…$` and `$$…$$`.
    pub math: bool,
    /// YAML front matter at the start (detected by `scan::front_matter_end`).
    pub front_matter: bool,
    /// GFM's tag filter: `<script>`, `<iframe>` and the like in raw HTML are
    /// shown as text. (Our sanitizer, ADR-0028, also removes them.)
    pub tagfilter: bool,
}

impl Flavor {
    /// What the app reads: CommonMark + GFM, front matter and math.
    pub const PRODUCT: Flavor = Flavor {
        table: true,
        strikethrough: true,
        autolink: true,
        tasklist: true,
        footnotes: true,
        math: true,
        front_matter: true,
        tagfilter: true,
    };

    /// Plain CommonMark.
    pub const COMMONMARK: Flavor = Flavor {
        table: false,
        strikethrough: false,
        autolink: false,
        tasklist: false,
        footnotes: false,
        math: false,
        front_matter: false,
        tagfilter: false,
    };

    /// CommonMark plus the GFM extensions named (cmark-gfm's names:
    /// `table`, `strikethrough`, `autolink`, `tagfilter`, `tasklist`,
    /// `footnotes`).
    pub fn gfm(names: &[&str]) -> Flavor {
        Flavor {
            table: names.contains(&"table"),
            strikethrough: names.contains(&"strikethrough"),
            autolink: names.contains(&"autolink"),
            tasklist: names.contains(&"tasklist"),
            footnotes: names.contains(&"footnotes"),
            tagfilter: names.contains(&"tagfilter"),
            ..Flavor::COMMONMARK
        }
    }
}

impl Default for Flavor {
    fn default() -> Self {
        Flavor::PRODUCT
    }
}

/// comrak options for parsing and rendering one block.
pub fn block(f: &Flavor) -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = f.table;
    o.extension.strikethrough = f.strikethrough;
    o.extension.autolink = f.autolink;
    o.extension.tasklist = f.tasklist;
    o.extension.footnotes = f.footnotes;
    o.extension.math_dollars = f.math;
    // Front matter never reaches comrak: the scanner finds it (ADR-0027).
    // Footnote definitions stay where they are written: blocks are parsed on
    // their own, and the document-wide footnote table numbers them.
    o.parse.leave_footnote_definitions = true;
    o.render.sourcepos = true;
    // Raw HTML is rendered, then sanitized by our allowlist (ADR-0028).
    o.render.r#unsafe = true;
    o
}
