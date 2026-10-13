# Markdown spec exceptions

The spec harness (`crates/markdown/tests/spec.rs`) runs every CommonMark and GFM spec example through the Markdown pipeline the app uses: the block index, per-block analysis, the document-wide tables and per-block rendering (ADR-0026, ADR-0027), before sanitization. Extensions are enabled per suite as cmark-gfm's runner enables them: none for CommonMark, the ones named in each example's header for the GFM spec, all of them for cmark-gfm's `extensions.txt`. Outputs are compared after the spec's normalization (`normalize.py`, ported to Rust), with one addition: `"` and `>` in text compare equal to `&quot;` and `&gt;`.

The corpora are fetched by `tools/corpora/fetch-markdown-spec.ps1` (CC-BY-SA 4.0, not committed). The test fails on any failure not listed here, and on a listed exception that starts passing.

## Results (2026-10-13)

| Suite | Examples | Pass | Rate |
|---|---|---|---|
| CommonMark 0.31.2 `spec.txt` | 652 | 652 | 100 % |
| GFM spec (cmark-gfm `test/spec.txt`; 2 disabled examples not run) | 670 | 661 | 98.66 % |
| cmark-gfm `test/extensions.txt` | 30 | 29 | 96.67 % |
| **All** | **1,352** | **1,342** | **99.26 %** |

## Exceptions

| Suite | Example | Reason |
|---|---|---|
| GFM | 398, 426, 434, 435, 436, 473, 474, 475, 477 | **Spec drift.** The GFM spec embeds CommonMark 0.29's text. CommonMark 0.30 changed how runs of `*` and `_` nest (`****foo****` is now `<strong><strong>foo</strong></strong>`). Each of these inputs is also a CommonMark 0.31.2 example (389, 417, 425, 426, 427, 464, 465, 466, 468) that expects the new result, and we pass all of those. No renderer can pass both versions. |
| GFM extensions | 19 | comrak doesn't link a URL whose host contains an emoji (`http://🍄.ga/`); the example's 30 other autolinks render as expected. |

## Not covered by the spec suites

- Front matter, `$` math and footnotes in the preview (definitions stay where they are written; exports collect them at the end as GitHub does) are tested by the crate's own unit tests and `tests/data/markdown/`.
- A label defined in two different blocks, used in the later one: the later block resolves it to its own definition, where CommonMark gives the earlier definition precedence. No spec example has this shape (ADR-0027).
