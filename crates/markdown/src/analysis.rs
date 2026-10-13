//! What a block contributes to the document-wide tables (ADR-0027): the link
//! reference definitions it makes, the footnotes it references and defines,
//! and its headings (the outline).
//!
//! comrak doesn't expose the definitions it collects, so they are found by a
//! probe: each candidate label (text before `]:`) is appended to the block as
//! a paragraph of its own, `[label]`, and the block is parsed without access
//! to other blocks' definitions. A probe that comes out as a link was defined
//! in the block, with that link's destination and title. Footnote references
//! only parse when their definition is present, so a stub definition is
//! appended for every `[^label]` the block mentions; real references then
//! show up as nodes, and the stubs are ignored.

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, parse_document};

use crate::options::{self, Flavor};
use crate::text::same_len_utf8;

/// A link reference definition's destination and title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkDef {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    /// Plain text of the heading's content.
    pub text: String,
    /// Line within the block, from 0.
    pub line: u32,
}

/// A block's contribution to the document-wide tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Link reference definitions in source order (first per label).
    pub defs: Vec<(String, LinkDef)>,
    /// Footnote labels referenced, in order of first reference, with the
    /// number of references to each.
    pub footnote_refs: Vec<(String, u32)>,
    /// Footnote labels defined.
    pub footnote_defs: Vec<String>,
    pub headings: Vec<Heading>,
}

impl Analysis {
    /// Whether the block takes part in no document-wide table.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
            && self.footnote_refs.is_empty()
            && self.footnote_defs.is_empty()
            && self.headings.is_empty()
    }
}

/// A label as CommonMark matches it: case-folded, inner whitespace collapsed,
/// trimmed.
pub fn normalize_label(label: &str) -> String {
    let mut v = String::with_capacity(label.len());
    let mut space = false;
    for c in label.trim().chars() {
        if c.is_whitespace() {
            if !space {
                v.push(' ');
            }
            space = true;
        } else {
            v.push(c);
            space = false;
        }
    }
    caseless::default_case_fold_str(&v)
}

/// Labels written as `[label]:`, possibly spanning lines (CommonMark allows
/// up to 999 characters), excluding footnotes (`[^label]:`).
fn definition_candidates(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(p) = text[i..].find("]:") {
        let close = i + p;
        i = close + 2;
        // Find the opening bracket: the nearest unescaped `[` before, with no
        // unescaped `]` between.
        let mut j = close;
        let floor = close.saturating_sub(1000);
        let mut open = None;
        while j > floor {
            j -= 1;
            let escaped = j > 0 && b[j - 1] == b'\\';
            match b[j] {
                b'[' if !escaped => {
                    open = Some(j);
                    break;
                }
                b']' if !escaped => break,
                _ => {}
            }
        }
        let Some(o) = open else { continue };
        let label = &text[o + 1..close];
        if label.starts_with('^') || label.trim().is_empty() {
            continue;
        }
        if !out.iter().any(|l| l == label) {
            out.push(label.to_owned());
        }
    }
    out
}

/// Footnote labels written as `[^label]`.
pub(crate) fn footnote_candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(p) = text[i..].find("[^") {
        let start = i + p + 2;
        i = start;
        let Some(len) = text[start..].find(']') else {
            break;
        };
        let label = &text[start..start + len];
        if label.is_empty() || label.len() > 999 || label.contains(['\n', '\r', '[']) {
            continue;
        }
        if !out.iter().any(|l| l == label) {
            out.push(label.to_owned());
        }
    }
    out
}

fn plain_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for d in node.descendants() {
        match &d.data().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::Math(m) => out.push_str(&m.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => {}
        }
    }
}

/// Analyzes one block's text.
pub fn analyze(bytes: &[u8], flavor: &Flavor) -> Analysis {
    let text = same_len_utf8(bytes);
    let defs = definition_candidates(&text);
    let notes = if flavor.footnotes {
        footnote_candidates(&text)
    } else {
        Vec::new()
    };
    let headings_possible = text.contains('#') || text.contains("\n=") || text.contains("\n-");
    if defs.is_empty() && notes.is_empty() && !headings_possible {
        return Analysis::default();
    }

    let mut probe = text.into_owned();
    if !probe.ends_with('\n') {
        probe.push('\n');
    }
    probe.push('\n');
    // Lines 1..=base belong to the block; probes come after.
    let base = probe.matches('\n').count();
    let mut probe_lines = Vec::with_capacity(defs.len());
    for label in &defs {
        probe_lines.push(probe.matches('\n').count() + 1);
        probe.push('[');
        probe.push_str(label);
        probe.push_str("]\n\n");
    }
    for label in &notes {
        probe.push_str("[^");
        probe.push_str(label);
        probe.push_str("]: \u{200b}\n\n");
    }

    let arena = Arena::new();
    let root = parse_document(&arena, &probe, &options::block(flavor));
    let mut out = Analysis::default();
    for node in root.descendants() {
        let d = node.data();
        let line = d.sourcepos.start.line;
        match &d.value {
            NodeValue::Link(l) if line > base => {
                if let Some(i) = probe_lines.iter().position(|&p| p == line) {
                    let key = normalize_label(&defs[i]);
                    if !out.defs.iter().any(|(k, _)| *k == key) {
                        out.defs.push((
                            key,
                            LinkDef {
                                url: l.url.clone(),
                                title: l.title.clone(),
                            },
                        ));
                    }
                }
            }
            NodeValue::FootnoteReference(r) if line <= base => {
                let key = normalize_label(&r.name);
                match out.footnote_refs.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => *n += 1,
                    None => out.footnote_refs.push((key, 1)),
                }
            }
            NodeValue::FootnoteDefinition(f) if line <= base => {
                let key = normalize_label(&f.name);
                if !out.footnote_defs.contains(&key) {
                    out.footnote_defs.push(key);
                }
            }
            NodeValue::Heading(h) if line <= base => {
                let mut t = String::new();
                plain_text(node, &mut t);
                out.headings.push(Heading {
                    level: h.level,
                    text: t.trim().to_owned(),
                    line: (line - 1) as u32,
                });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_definitions_with_titles_and_multiline_labels() {
        let a = analyze(
            b"[Foo  Bar]: /url \"T\"\n[multi\nline]:\n  /m\n\nText [foo bar].\n",
            &Flavor::PRODUCT,
        );
        assert_eq!(
            a.defs,
            vec![
                (
                    "foo bar".into(),
                    LinkDef {
                        url: "/url".into(),
                        title: "T".into()
                    }
                ),
                (
                    "multi line".into(),
                    LinkDef {
                        url: "/m".into(),
                        title: String::new()
                    }
                ),
            ]
        );
    }

    #[test]
    fn definitions_inside_containers_count_and_code_does_not() {
        let a = analyze(
            b"> [q]: /quoted\n\n- [l]: /listed\n\n```\n[c]: /code\n```\n",
            &Flavor::PRODUCT,
        );
        let keys: Vec<&str> = a.defs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["q", "l"]);
    }

    #[test]
    fn not_a_definition_without_destination() {
        assert!(analyze(b"[x]: \n", &Flavor::PRODUCT).defs.is_empty());
        assert!(analyze(b"text [x]: /u\n", &Flavor::PRODUCT).defs.is_empty());
    }

    #[test]
    fn footnotes_and_headings() {
        let a = analyze(
            b"# Title *em*\n\nSee[^a] and [^B] and `[^c]` and [^a].\n\n[^b]: def\n",
            &Flavor::PRODUCT,
        );
        assert_eq!(a.footnote_refs, vec![("a".into(), 2), ("b".into(), 1)]);
        assert_eq!(a.footnote_defs, vec!["b"]);
        assert_eq!(
            a.headings,
            vec![Heading {
                level: 1,
                text: "Title em".into(),
                line: 0
            }]
        );
        let a = analyze(b"Setext\n======\n", &Flavor::PRODUCT);
        assert_eq!(a.headings[0].level, 1);
        assert!(analyze(b"plain paragraph\n", &Flavor::PRODUCT).is_empty());
    }
}
