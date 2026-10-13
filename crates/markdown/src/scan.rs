//! Top-level block boundaries from a parse of a stretch of the document.
//!
//! A stretch is parsed from a position where a top-level block starts. Every
//! top-level block in it except the last is final: what follows can't change
//! a block once another block has started after it (a setext underline or a
//! lazy continuation line only extends the block that is still open, which is
//! the last one). So the scanner keeps all but the last block and the next
//! stretch starts where the last block starts. At the end of the document the
//! last block is final too.

use std::ops::Range;

use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};

use crate::options;
use crate::text::{line_starts, same_len_utf8};

/// What a top-level block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    FrontMatter,
    Paragraph,
    Heading {
        level: u8,
        setext: bool,
    },
    ThematicBreak,
    CodeBlock {
        fenced: bool,
        info: String,
    },
    HtmlBlock,
    List,
    BlockQuote,
    Table,
    FootnoteDefinition {
        label: String,
    },
    /// Display math (`$$`) on its own; inline math is part of a paragraph.
    Math,
    /// Lines with no node of their own: link reference definitions (comrak
    /// consumes them while parsing paragraphs).
    Definitions,
    Other,
}

fn kind_of(v: &NodeValue) -> BlockKind {
    match v {
        NodeValue::FrontMatter(_) => BlockKind::FrontMatter,
        NodeValue::Paragraph => BlockKind::Paragraph,
        NodeValue::Heading(h) => BlockKind::Heading {
            level: h.level,
            setext: h.setext,
        },
        NodeValue::ThematicBreak => BlockKind::ThematicBreak,
        NodeValue::CodeBlock(c) => BlockKind::CodeBlock {
            fenced: c.fenced,
            info: c.info.clone(),
        },
        NodeValue::HtmlBlock(_) => BlockKind::HtmlBlock,
        NodeValue::List(_) => BlockKind::List,
        NodeValue::BlockQuote => BlockKind::BlockQuote,
        NodeValue::Table(_) => BlockKind::Table,
        NodeValue::FootnoteDefinition(f) => BlockKind::FootnoteDefinition {
            label: f.name.clone(),
        },
        NodeValue::Math(_) => BlockKind::Math,
        _ => BlockKind::Other,
    }
}

/// A top-level block found by a scan: byte range relative to the scanned
/// stretch, from the start of its first line to the start of the next block
/// (blank lines and link reference definitions after it included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub range: Range<usize>,
    pub kind: BlockKind,
    /// A paragraph that starts with `[` and may still turn out to be only
    /// link reference definitions (no block at all) when lines after the
    /// stretch join it: a label or title can span lines.
    pub may_vanish: bool,
}

fn is_blank(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}
/// Front matter must close within this many bytes of the start; a longer
/// block is shown as Markdown (a thematic break and the text after it).
pub const FRONT_MATTER_MAX: u64 = 1 << 20;

/// End of the YAML front matter at the start of the text, if there is one: an
/// opening line `---` (after an optional BOM), then everything up to and
/// including the first closing line `---` (end of the text counts as a line
/// end). Detected here rather than by comrak, whose rule needs a search of the
/// whole text (ADR-0027).
pub fn front_matter_end<T: crate::text::Text + ?Sized>(text: &T) -> std::io::Result<Option<u64>> {
    front_matter_end_within(text, FRONT_MATTER_MAX)
}

/// As [`front_matter_end`], looking only at the first `limit` bytes (at most
/// [`FRONT_MATTER_MAX`]); a closing line cut by the limit doesn't count.
pub fn front_matter_end_within<T: crate::text::Text + ?Sized>(
    text: &T,
    limit: u64,
) -> std::io::Result<Option<u64>> {
    let head = text.read(0..limit.min(FRONT_MATTER_MAX).min(text.len()))?;
    let bom = if head.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    let body = &head[bom..];
    let first = match body {
        [b'-', b'-', b'-', b'\n', ..] => 4,
        [b'-', b'-', b'-', b'\r', b'\n', ..] => 5,
        _ => return Ok(None),
    };
    let starts = line_starts(&body[first..]);
    let lines_end = body.len() - first;
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(lines_end);
        let line = &body[first + s..first + e];
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let complete = e < lines_end || (head.len() as u64) == text.len();
        if content == b"---" && complete {
            return Ok(Some((bom + first + e) as u64));
        }
    }
    Ok(None)
}

/// Parses `bytes` (a stretch that starts where a top-level block starts, or
/// at the start of the document after any front matter) and returns its
/// top-level blocks in order. The blocks tile the stretch: each runs to the
/// start of the next, and the first starts at the stretch's start.
///
/// Only start lines are used. comrak sets them when a block opens, so they
/// are exact; its end positions are not always (an unclosed fence inside a
/// list item ended by a blank line reports its first line as its end).
pub fn scan(bytes: &[u8]) -> Vec<Found> {
    let text = same_len_utf8(bytes);
    let arena = Arena::new();
    let root = parse_document(&arena, &text, &options::block());
    let starts = line_starts(bytes);
    let len = bytes.len();

    let mut nodes: Vec<(usize, BlockKind)> = root
        .children()
        .map(|n| {
            let d = n.data();
            (d.sourcepos.start.line, kind_of(&d.value))
        })
        .collect();
    nodes.sort_by_key(|n| n.0);
    nodes.dedup_by_key(|n| n.0);

    let line_at = |line: usize| starts.get(line.saturating_sub(1)).copied().unwrap_or(len);
    // A table whose header row was the last line of an open paragraph (GFM
    // lets a table interrupt a paragraph that way) doesn't start from a fresh
    // parser state: parsed from its own start, its header would be an
    // indented code block or a lazy line. It stays in the paragraph's block.
    let mut i = 1;
    while i < nodes.len() {
        let split_from_paragraph = nodes[i].1 == BlockKind::Table
            && nodes[i - 1].1 == BlockKind::Paragraph
            && !is_blank(&bytes[line_at(nodes[i].0 - 1)..line_at(nodes[i].0)]);
        if split_from_paragraph {
            nodes.remove(i);
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    for (i, (line, kind)) in nodes.iter().enumerate() {
        let a = if i == 0 { 0 } else { line_at(*line) };
        let b = nodes.get(i + 1).map_or(len, |n| line_at(n.0));
        if b > a {
            let first_line = &bytes[line_at(*line)..b];
            let indent = first_line
                .iter()
                .take(3)
                .take_while(|&&c| c == b' ')
                .count();
            out.push(Found {
                range: a..b,
                kind: kind.clone(),
                may_vanish: *kind == BlockKind::Paragraph && first_line.get(indent) == Some(&b'['),
            });
        }
    }
    // Text with no node at all: only link reference definitions.
    if out.is_empty() && !is_blank(bytes) {
        out.push(Found {
            range: 0..len,
            kind: BlockKind::Definitions,
            may_vanish: false,
        });
    }
    out
}
/// Every top-level block of a whole text, front matter included: the
/// reference the chunked index is tested against.
pub fn scan_document(bytes: &[u8]) -> Vec<Found> {
    let fm = front_matter_end(bytes).ok().flatten().unwrap_or(0) as usize;
    let mut out = Vec::new();
    if fm > 0 {
        out.push(Found {
            range: 0..fm,
            kind: BlockKind::FrontMatter,
            may_vanish: false,
        });
    }
    out.extend(scan(&bytes[fm..]).into_iter().map(|f| Found {
        range: f.range.start + fm..f.range.end + fm,
        ..f
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(s: &str) -> Vec<(&str, BlockKind)> {
        scan_document(s.as_bytes())
            .into_iter()
            .map(|f| (&s[f.range], f.kind))
            .collect()
    }

    #[test]
    fn front_matter_rules() {
        let fm = |s: &str| front_matter_end(s.as_bytes()).unwrap();
        assert_eq!(fm("---\na: 1\n---\nrest"), Some(13));
        assert_eq!(fm("---\r\na: 1\r\n---\r\n"), Some(16));
        assert_eq!(fm("---\na: 1\n---"), Some(12));
        assert_eq!(fm("\u{feff}---\n---\n"), Some(11));
        assert_eq!(fm("---\na: 1\n"), None);
        assert_eq!(fm("--- \na\n---\n"), None);
        assert_eq!(fm("text\n---\n"), None);
    }

    #[test]
    fn simple_blocks() {
        let s = "# Title\n\nPara one\nstill one\n\n- a\n- b\n\n  c\n\nlast";
        let b = blocks(s);
        assert_eq!(
            b,
            vec![
                (
                    "# Title\n\n",
                    BlockKind::Heading {
                        level: 1,
                        setext: false
                    }
                ),
                ("Para one\nstill one\n\n", BlockKind::Paragraph),
                ("- a\n- b\n\n  c\n\n", BlockKind::List),
                ("last", BlockKind::Paragraph),
            ]
        );
    }

    #[test]
    fn definitions_and_footnotes_stay_in_blocks() {
        let s = "[a]: /x\n[b]: /y\n\nText [a].\n\n[c]: /z\n\n[^n]: Note.\n";
        let b = blocks(s);
        assert_eq!(
            b[0],
            (
                "[a]: /x\n[b]: /y\n\nText [a].\n\n[c]: /z\n\n",
                BlockKind::Paragraph
            )
        );
        assert_eq!(
            b[1],
            (
                "[^n]: Note.\n",
                BlockKind::FootnoteDefinition { label: "n".into() }
            )
        );
        assert_eq!(
            blocks("[a]: /x\n"),
            vec![("[a]: /x\n", BlockKind::Definitions)]
        );
        assert_eq!(blocks("\n\n"), vec![]);
    }

    #[test]
    fn front_matter_fences_and_crlf() {
        let s = "---\ntitle: x\n---\n\n```rust\nfn f() {}\n\n```\r\n\r\npara\r\n";
        let b = blocks(s);
        assert_eq!(b[0], ("---\ntitle: x\n---\n", BlockKind::FrontMatter));
        assert_eq!(
            b[1],
            (
                "\n```rust\nfn f() {}\n\n```\r\n\r\n",
                BlockKind::CodeBlock {
                    fenced: true,
                    info: "rust".into()
                }
            )
        );
        assert_eq!(b[2], ("para\r\n", BlockKind::Paragraph));
    }

    #[test]
    fn blocks_tile_the_text() {
        let s = "a\n\n\n> q\n> r\n\n| x |\n|---|\n| 1 |\n\n***\n\n$$\nx\n$$\n\n<div>\nhi\n</div>\n";
        let b = scan(s.as_bytes());
        assert_eq!(b[0].range.start, 0);
        for w in b.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
        }
        assert_eq!(b.last().unwrap().range.end, s.len());
    }

    #[test]
    fn a_table_split_from_a_paragraph_stays_with_it() {
        let s = "$$\n\t<x$$\n|---|\n\nafter\n";
        assert_eq!(
            blocks(s),
            vec![
                ("$$\n\t<x$$\n|---|\n\n", BlockKind::Paragraph),
                ("after\n", BlockKind::Paragraph)
            ]
        );
        // After a heading, a table starts fresh.
        let s = "## h\n| a |\n|---|\n";
        assert_eq!(blocks(s)[1], ("| a |\n|---|\n", BlockKind::Table));
    }

    #[test]
    fn list_runs_to_the_next_block() {
        // comrak reports these lists as ending on their first line.
        let s = "- ```para\r\n  # | a | b |\n\n```\n";
        assert_eq!(
            blocks(s)[0],
            ("- ```para\r\n  # | a | b |\n\n", BlockKind::List)
        );
        let s = "- ```p<details>\n  # ***\n\n`<details>\n";
        assert_eq!(
            blocks(s)[0],
            ("- ```p<details>\n  # ***\n\n", BlockKind::List)
        );
    }
}
