//! Document-wide tables built from the blocks' analyses, in document order
//! (ADR-0027): link reference definitions (the first definition of a label
//! wins), footnote numbers (by first reference, as GitHub numbers them) and
//! the outline with unique heading ids.

use std::collections::HashMap;

use crate::analysis::{Analysis, LinkDef};

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
    /// Builds the tables from the analyzed blocks in document order:
    /// (block id, its analysis) for the blocks that have one (the others take
    /// part in no table); `complete` says whether every block was analyzed.
    pub fn build<'a>(blocks: impl Iterator<Item = (u64, &'a Analysis)>, complete: bool) -> Self {
        let mut t = Tables {
            complete,
            ..Tables::default()
        };
        // Keys are cloned only when first inserted (this runs over the whole
        // document after edits that change what a block contributes).
        let mut refs_in_order: Vec<&'a str> = Vec::new();
        let mut seen_slugs: HashMap<&'a str, u32> = HashMap::new();
        for (id, a) in blocks {
            for (k, d) in &a.defs {
                if !t.defs.contains_key(k.as_str()) {
                    t.defs.insert(k.clone(), d.clone());
                }
            }
            for k in &a.footnote_defs {
                if !t.footnote_defined.contains_key(k.as_str()) {
                    t.footnote_defined.insert(k.clone(), id);
                }
            }
            for (k, n) in &a.footnote_refs {
                match t.footnote_refs_total.get_mut(k.as_str()) {
                    Some(total) => {
                        t.footnote_refs_before.insert((id, k.clone()), *total);
                        *total += n;
                    }
                    None => {
                        t.footnote_refs_total.insert(k.clone(), *n);
                    }
                }
                refs_in_order.push(k);
            }
            t.add_headings(id, a, &mut seen_slugs);
        }
        let mut next = 1;
        for k in refs_in_order {
            if t.footnote_defined.contains_key(k) && !t.footnote_numbers.contains_key(k) {
                t.footnote_numbers.insert(k.to_owned(), next);
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

    /// Block `old` was replaced by block `new` with an equal analysis `a`:
    /// the tables stay the same but for the id (typing in a paragraph or a
    /// heading's block that leaves its contribution unchanged).
    pub fn rename(&mut self, old: u64, new: u64, a: &Analysis) {
        for k in &a.footnote_defs {
            if let Some(id) = self.footnote_defined.get_mut(k)
                && *id == old
            {
                *id = new;
            }
        }
        for (k, _) in &a.footnote_refs {
            if let Some(n) = self.footnote_refs_before.remove(&(old, k.clone())) {
                self.footnote_refs_before.insert((new, k.clone()), n);
            }
        }
        for i in 0..a.headings.len() {
            if let Some(at) = self.slugs.remove(&(old, i)) {
                self.outline[at].block = new;
                self.slugs.insert((new, i), at);
            }
        }
    }

    /// Rebuilds only the outline (headings changed, nothing else did).
    pub fn rebuild_outline<'a>(&mut self, blocks: impl Iterator<Item = (u64, &'a Analysis)>) {
        self.outline.clear();
        self.slugs.clear();
        let mut seen_slugs: HashMap<&'a str, u32> = HashMap::new();
        for (id, a) in blocks {
            self.add_headings(id, a, &mut seen_slugs);
        }
    }

    fn add_headings<'a>(
        &mut self,
        block: u64,
        a: &'a Analysis,
        seen_slugs: &mut HashMap<&'a str, u32>,
    ) {
        for (i, h) in a.headings.iter().enumerate() {
            let base = h.slug.as_str();
            let n = seen_slugs.entry(base).or_insert(0);
            let s = if *n == 0 {
                base.to_owned()
            } else {
                format!("{base}-{n}")
            };
            *n += 1;
            self.slugs.insert((block, i), self.outline.len());
            self.outline.push(OutlineEntry {
                block,
                index: i,
                level: h.level,
                text: h.text.clone(),
                slug: s,
            });
        }
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

    /// Everything in the tables with block ids replaced by positions in
    /// `blocks`, sorted: equal for equal documents however they were built.
    #[cfg(test)]
    pub(crate) fn summary(&self, blocks: &[crate::index::Block]) -> String {
        let pos = |id: u64| blocks.iter().position(|b| b.id == id);
        let mut defs: Vec<_> = self
            .defs
            .iter()
            .map(|(k, d)| format!("{k}={}|{}", d.url, d.title))
            .collect();
        defs.sort();
        let mut numbers: Vec<_> = self
            .footnote_numbers
            .iter()
            .map(|(k, n)| format!("{k}#{n}"))
            .collect();
        numbers.sort();
        let mut defined: Vec<_> = self
            .footnote_defined
            .iter()
            .map(|(k, id)| format!("{k}@{:?}", pos(*id)))
            .collect();
        defined.sort();
        let mut before: Vec<_> = self
            .footnote_refs_before
            .iter()
            .map(|((id, k), n)| format!("{:?}:{k}={n}", pos(*id)))
            .collect();
        before.sort();
        let mut total: Vec<_> = self
            .footnote_refs_total
            .iter()
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        total.sort();
        let outline: Vec<_> = self
            .outline
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let linked = self.slugs.get(&(e.block, e.index)) == Some(&i);
                format!(
                    "{:?}/{} h{} {} #{} {linked}",
                    pos(e.block),
                    e.index,
                    e.level,
                    e.text,
                    e.slug
                )
            })
            .collect();
        format!(
            "defs {defs:?}
numbers {numbers:?}
defined {defined:?}
before {before:?}
total {total:?}
outline {outline:?}
slugs {}",
            self.slugs.len()
        )
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
        let t = Tables::build(
            ix.blocks()
                .iter()
                .filter_map(|b| analyses.get(&b.id).map(|a| (b.id, a))),
            true,
        );
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
