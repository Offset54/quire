# Test corpora

Real-world test inputs that can't be committed (MASTER_PROMPT §10: track corpus licenses; fetch what can't be redistributed). Each script fetches pinned revisions into `.cache/corpora/`, which is ignored, and checks SHA-256 sums.

| Corpus | Script | Source | License | Used by |
|---|---|---|---|---|
| CommonMark spec 0.31.2 (`spec.txt`, 652 examples) | `fetch-markdown-spec.ps1` | [commonmark/commonmark-spec](https://github.com/commonmark/commonmark-spec) tag `0.31.2` | CC-BY-SA 4.0 | `crates/markdown/tests/spec.rs` |
| GFM spec (`test/spec.txt`, 672 examples, 2 disabled) | `fetch-markdown-spec.ps1` | [github/cmark-gfm](https://github.com/github/cmark-gfm) at `27d942c8` | CC-BY-SA 4.0 | `crates/markdown/tests/spec.rs` |
| cmark-gfm extension tests (`test/extensions.txt`, 30 examples) | `fetch-markdown-spec.ps1` | [github/cmark-gfm](https://github.com/github/cmark-gfm) at `27d942c8` | CC-BY-SA 4.0 | `crates/markdown/tests/spec.rs` |
