//! comrak options: the extensions §3.1 lists (ADR-0026).

use comrak::Options;

/// Options for parsing and rendering a block.
pub fn block() -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    // Front matter is detected by `scan::front_matter_end` and never reaches
    // comrak (ADR-0027).
    o.extension.math_dollars = true;
    // Footnote definitions stay where they are written: blocks are parsed on
    // their own, and the document-wide footnote table numbers them.
    o.parse.leave_footnote_definitions = true;
    o.render.sourcepos = true;
    // Raw HTML is rendered, then sanitized by our allowlist (ADR-0028).
    o.render.r#unsafe = true;
    o
}
