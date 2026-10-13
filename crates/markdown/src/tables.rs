//! Document-wide tables built from the blocks' analyses, in document order
//! (ADR-0027): link reference definitions (the first definition of a label
//! wins), footnote numbers (by first reference, as GitHub numbers them) and
//! the outline with unique heading ids.

use std::collections::HashMap;

use crate::analysis::{Analysis, LinkDef};
use crate::index::Block;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    pub block: u64,
    /// Heading within its block, from 0.
    pub index: usize,
    pub level: u8,
    pub text: String,
    /// Unique id (GitHub's slug rules).
    pub slug: String,
}

#[derive(Clone, Debug, Default)]
pub struct Tables {
    defs: HashMap<String, LinkDef>,
    /// Footnote label → number, for labels that are both defined and referenced.
    footnote_numbers: HashMap<String, u32>,
    footnote_defined: HashMap<String, u64>,
    /// References to a footnote before a block: (block, label) → count.
    footnote_refs_before: HashMap<(u64, String), u32>,
    /// All references to a footnote.
    footnote_refs_total: HashMap<String, u32>,
    outline: Vec<OutlineEntry>,
    slugs: HashMap<(u64, usize), usize>,
    /// Every block was analyzed: nothing defined later can still appear.
    pub complete: bool,
}

/// GitHub's heading id: lowercase, punctuation other than `-` and `_`
/// removed, spaces as `-`.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

impl Tables {
    /// Builds the tables from the blocks in order. `analysis(id)` gives a
    /// block's analysis if it has one (blocks without any take part in no
    /// table); `complete` says whether every block was analyzed.
    pub fn build<'a>(
        blocks: &[Block],
        analysis: impl Fn(u64) -> Option<&'a Analysis>,
        complete: bool,
    ) -> Self {
        let mut t = Tables {
            complete,
            ..Tables::default()
        };
        let mut refs_in_order: Vec<String> = Vec::new();
        let mut seen_slugs: HashMap<String, u32> = HashMap::new();
        for b in blocks {
            let Some(a) = analysis(b.id) else { continue };
            for (k, d) in &a.defs {
                t.defs.entry(k.clone()).or_insert_with(|| d.clone());
            }
            for k in &a.footnote_defs {
                t.footnote_defined.entry(k.clone()).or_insert(b.id);
            }
            for (k, n) in &a.footnote_refs {
                let total = t.footnote_refs_total.entry(k.clone()).or_insert(0);
                if *total > 0 {
                    t.footnote_refs_before.insert((b.id, k.clone()), *total);
                }
                *total += n;
                refs_in_order.push(k.clone());
            }
            for (i, h) in a.headings.iter().enumerate() {
                let base = slug(&h.text);
                let n = seen_slugs.entry(base.clone()).or_insert(0);
                let s = if *n == 0 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                };
                *n += 1;
                t.slugs.insert((b.id, i), t.outline.len());
                t.outline.push(OutlineEntry {
                    block: b.id,
                    index: i,
                    level: h.level,
                    text: h.text.clone(),
                    slug: s,
                });
            }
        }
        let mut next = 1;
        for k in refs_in_order {
            if t.footnote_defined.contains_key(&k) && !t.footnote_numbers.contains_key(&k) {
                t.footnote_numbers.insert(k, next);
                next += 1;
            }
        }
        t
    }

    /// The winning definition of a (normalized) label.
    pub fn definition(&self, key: &str) -> Option<&LinkDef> {
        self.defs.get(key)
    }

    /// A footnote's number, if it is defined and referenced.
    pub fn footnote_number(&self, key: &str) -> Option<u32> {
        self.footnote_numbers.get(key).copied()
    }

    pub fn footnote_defined(&self, key: &str) -> bool {
        self.footnote_defined.contains_key(key)
    }

    /// References to a footnote in the blocks before `block`.
    pub fn footnote_refs_before(&self, block: u64, key: &str) -> u32 {
        self.footnote_refs_before
            .get(&(block, key.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    /// References to a footnote in the whole document.
    pub fn footnote_refs_total(&self, key: &str) -> u32 {
        self.footnote_refs_total.get(key).copied().unwrap_or(0)
    }

    /// Footnote labels in number order.
    pub fn footnotes_in_order(&self) -> Vec<(&str, u32)> {
        let mut v: Vec<(&str, u32)> = self
            .footnote_numbers
            .iter()
            .map(|(k, n)| (k.as_str(), *n))
            .collect();
        v.sort_by_key(|x| x.1);
        v
    }

    pub fn outline(&self) -> &[OutlineEntry] {
        &self.outline
    }

    /// Id of a block's `index`-th heading.
    pub fn heading_slug(&self, block: u64, index: usize) -> Option<&str> {
        self.slugs
            .get(&(block, index))
            .map(|&i| self.outline[i].slug.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::index::BlockIndex;

    fn tables(doc: &str) -> (BlockIndex, Tables) {
        let mut ix = BlockIndex::new(doc.len() as u64);
        ix.extend_to(doc.as_bytes(), doc.len() as u64).unwrap();
        let analyses: HashMap<u64, Analysis> = ix
            .blocks()
            .iter()
            .map(|b| {
                let bytes = &doc.as_bytes()[b.start as usize..b.end as usize];
                (b.id, analyze(bytes, &crate::options::Flavor::PRODUCT))
            })
            .collect();
        let t = Tables::build(ix.blocks(), |id| analyses.get(&id), true);
        (ix, t)
    }

    #[test]
    fn first_definition_wins_and_footnotes_number_by_reference() {
        let (_, t) = tables(
            "Uses [x] and[^b] then[^a].\n\n[x]: /first\n\n# Head\n\n[X]: /second\n\n[^a]: A\n\n[^b]: B\n\n[^c]: unused\n",
        );
        assert_eq!(t.definition("x").unwrap().url, "/first");
        assert_eq!(t.footnote_number("b"), Some(1));
        assert_eq!(t.footnote_number("a"), Some(2));
        assert_eq!(t.footnote_number("c"), None);
        assert!(t.footnote_defined("c"));
    }

    #[test]
    fn outline_slugs_are_unique() {
        let (ix, t) = tables("# Hello, World!\n\n## Hello World\n\n# hello world\n");
        let slugs: Vec<&str> = t.outline().iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["hello-world", "hello-world-1", "hello-world-2"]);
        assert_eq!(t.heading_slug(ix.blocks()[1].id, 0), Some("hello-world-1"));
        assert_eq!(slug("Ünïcode & 한글 test_1"), "ünïcode--한글-test_1");
    }
}
