//! Code highlighting with tree-sitter (ADR-0029).
//!
//! The languages are M2's decision D. [`highlight`] turns a code block's text
//! into HTML: the text escaped, with `<span class="hl-…">` around what the
//! grammar's highlight query captures. Blocks over [`MAX_BYTES`], or with a
//! line over [`MAX_LINE_BYTES`], aren't highlighted (§2: degrade honestly;
//! the caller labels them). Each grammar's configuration is built once, on
//! first use. [`service`] does the work on a background thread, with a
//! cache, for callers that can't wait.

mod languages;
pub mod service;

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use tree_sitter_highlight::{HighlightEvent, Highlighter};

pub use languages::{LANGUAGES, Language, language};

/// Code larger than this isn't highlighted.
pub const MAX_BYTES: usize = 256 * 1024;
/// Nor is code with a line longer than this.
pub const MAX_LINE_BYTES: usize = 16 * 1024;
/// Code up to this size may be highlighted on the caller's thread when it
/// must not flash plain (an edited block): a few milliseconds at most
/// (`examples/hlbench.rs`).
pub const INLINE_MAX_BYTES: usize = 4 * 1024;

/// Capture names we style, in order (their index is the class). A capture
/// such as `function.method.call` takes the longest name here that is a
/// prefix of it (`function.method`).
pub const NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "boolean",
    "character",
    "markup.heading",
    "markup.bold",
    "markup.italic",
    "markup.link",
    "markup.raw",
    "markup.list",
    "markup.quote",
];

/// The CSS class for a name in [`NAMES`]: `hl-` and the name with dots as
/// dashes.
pub fn class_of(name: &str) -> String {
    format!("hl-{}", name.replace('.', "-"))
}

/// Why a block wasn't highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plain {
    /// No grammar for the language (or none given).
    Unknown,
    /// Over [`MAX_BYTES`] or [`MAX_LINE_BYTES`].
    TooLarge,
    /// The highlighter failed or was cancelled.
    Failed,
}

static HIGHLIGHTED: AtomicUsize = AtomicUsize::new(0);

/// Blocks highlighted by this process (tests and benchmarks).
pub fn highlighted_count() -> usize {
    HIGHLIGHTED.load(Ordering::Relaxed)
}

thread_local! {
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

/// Whether `code` is within the size cap.
pub fn within_cap(code: &str) -> bool {
    code.len() <= MAX_BYTES && code.split('\n').all(|l| l.len() <= MAX_LINE_BYTES)
}

/// `code` as HTML with highlight spans, for the info string's first word
/// `lang` (a name or an alias, any case).
pub fn highlight(lang: &str, code: &str) -> Result<String, Plain> {
    let l = language(lang).ok_or(Plain::Unknown)?;
    if !within_cap(code) {
        return Err(Plain::TooLarge);
    }
    let config = l.config();
    let html = HIGHLIGHTER.with(|h| {
        let mut h = h.borrow_mut();
        let events = h
            .highlight(config, code.as_bytes(), None, None, |name| {
                language(name).map(Language::config)
            })
            .map_err(|_| Plain::Failed)?;
        let mut out = String::with_capacity(code.len() * 2);
        let mut open = 0usize;
        for e in events {
            match e.map_err(|_| Plain::Failed)? {
                HighlightEvent::Source { start, end } => escape(&code[start..end], &mut out),
                HighlightEvent::HighlightStart(hl) => {
                    out.push_str("<span class=\"");
                    out.push_str(&class_of(NAMES[hl.0]));
                    out.push_str("\">");
                    open += 1;
                }
                HighlightEvent::HighlightEnd => {
                    out.push_str("</span>");
                    open = open.saturating_sub(1);
                }
            }
        }
        for _ in 0..open {
            out.push_str("</span>");
        }
        Ok(out)
    })?;
    HIGHLIGHTED.fetch_add(1, Ordering::Relaxed);
    Ok(html)
}

/// HTML-escapes `s` onto `out` (text content).
pub fn escape(s: &str, out: &mut String) {
    let mut last = 0;
    for (i, c) in s.char_indices() {
        let rep = match c {
            '<' => "&lt;",
            '>' => "&gt;",
            '&' => "&amp;",
            '"' => "&quot;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                c if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
    }

    #[test]
    fn every_language_highlights_its_sample() {
        for l in &LANGUAGES {
            let html =
                highlight(l.names[0], l.sample).unwrap_or_else(|e| panic!("{}: {e:?}", l.names[0]));
            assert!(html.contains("<span class=\"hl-"), "{}: {html}", l.names[0]);
            assert_eq!(text_of(&html), l.sample, "{}: text changed", l.names[0]);
            assert_eq!(
                html.matches("<span").count(),
                html.matches("</span>").count(),
                "{}",
                l.names[0]
            );
        }
    }

    #[test]
    fn aliases_and_case() {
        for name in [
            "rs", "Rust", "py", "ts", "tsx", "sh", "ps1", "yml", "c++", "cs", "kt", "md",
        ] {
            assert!(language(name).is_some(), "{name}");
        }
        assert_eq!(highlight("cobol", "x"), Err(Plain::Unknown));
        assert_eq!(highlight("", "x"), Err(Plain::Unknown));
    }

    #[test]
    fn caps() {
        let long_line = "a".repeat(MAX_LINE_BYTES + 1);
        assert_eq!(highlight("rust", &long_line), Err(Plain::TooLarge));
        let big = "let x = 1;\n".repeat(MAX_BYTES / 11 + 1);
        assert_eq!(highlight("rust", &big), Err(Plain::TooLarge));
    }

    #[test]
    fn escapes_markup() {
        let html = highlight("html", "<script>alert(\"&\")</script>\n").unwrap();
        assert!(!html.contains("<script"));
        assert_eq!(text_of(&html), "<script>alert(\"&\")</script>\n");
    }

    #[test]
    fn injections_highlight_embedded_code() {
        // JavaScript inside HTML.
        let html = highlight("html", "<script>const x = 1;</script>\n").unwrap();
        assert!(html.contains("hl-keyword"), "{html}");
    }

    #[test]
    fn invalid_code_still_round_trips() {
        let code = "fn (( {{ \"unterminated\n";
        let html = highlight("rust", code).unwrap();
        assert_eq!(text_of(&html), code);
    }
}
