# 0026. Markdown parser: comrak

- Status: Accepted
- Date: 2026-10-13

## Context

§3.1 asks for CommonMark plus GFM (tables, task lists, footnotes, strikethrough, autolinks), YAML front matter and KaTeX math; §11's M2 DoD asks for at least 99 % of the CommonMark and GFM spec tests; §6.2 needs a source map (block ↔ line range) for scroll sync and click-to-source, and incremental re-parsing of the blocks an edit touches. §5.1 names two candidates, `comrak` and `pulldown-cmark`, both permissive. The M2 plan's task 1 compares them on spec pass rates as shipped, throughput on the 20 MiB fixture, source positions, the extensions we need, and parsing a block range in isolation.

## Evidence

Spike: `spikes/md-parser` (not shipped). Results: `bench/results/m2/parser-spike-2026-10-13.json`, dev machine raw (not ref-sim), AC power.

**Spec tests.** CommonMark 0.31.2 (`spec.txt`, 652 examples), the GFM spec (cmark-gfm `test/spec.txt`, 670 enabled examples) and cmark-gfm's `test/extensions.txt` (30). Extensions per example as cmark-gfm's runner enables them (from the example header for the GFM spec, all of them for extensions.txt). Outputs are compared after normalization by a Rust port of the spec's `normalize.py`, extended so that `"` and `>` in text equal their character references (otherwise pulldown-cmark, which writes them raw, fails 21 CommonMark examples on serialization alone).

| Suite | Examples | comrak 0.56.0 | pulldown-cmark 0.13.4 |
|---|---|---|---|
| CommonMark 0.31.2 | 652 | 652 (100 %) | 652 (100 %) |
| GFM spec | 670 | 660 (98.5 %) | 647 (96.6 %) |
| GFM extensions.txt | 30 | 28 (93.3 %) | 20 (66.7 %) |

What fails:
- **Both, 9 GFM examples** (398, 426, 434–436, 473–475, 477): emphasis. The GFM spec embeds CommonMark 0.29's text; 0.31.2 changed these results (`****foo****` → nested `<strong>`), and both parsers follow 0.31.2, which they pass completely.
- **Both, tag filter** (GFM 652, extensions 22): comrak 0.56.0 removed its `tagfilter` extension; pulldown-cmark never had one. Our allowlist sanitizer (ADR-0028) removes those tags instead of escaping them.
- **comrak, 1:** extensions 19, an emoji host name (`http://🍄.ga/`) is not linked.
- **pulldown-cmark, 11 + 8:** no GFM extended autolinks (`www.`, bare URLs and e-mail addresses: GFM 621–631, extensions 19, 20, 26, 27); table and footnote markup that differs from GitHub's (empty `<tbody>`, `style` instead of `align`, footnote definitions in place rather than in a section). The second group is renderer formatting; the first is a missing feature we would have to write.

**Throughput on the 20 MiB fixture** (median of 5):

| | comrak | pulldown-cmark |
|---|---|---|
| Parse only | 693 ms (30 MB/s; 2.1 M AST nodes) | 230 ms (91 MB/s; 3.0 M events) |
| Parse + HTML | 884 ms | 294 ms |
| Top-level blocks (210,938) | 640 ms | 239 ms |
| One top-level block in isolation, p50 / p95 | 2.1 / 13.1 µs | 1.0 / 4.0 µs |

Neither parses the whole file inside the first-paint budget (500 ms, ≈ 287 ms effective in ref-sim), so first paint must parse only the blocks around the viewport whichever parser we use (ADR-0027). Whole-file parsing is background work, where comrak's extra 0.45 s is not on any §4 row. A block in isolation costs microseconds with either, far inside the 33 ms keystroke budget.

**Source positions.** comrak records line:column spans on every node, blocks and inlines (`Heading 6:1-6:15 > Emph 6:11-6:15`), and its HTML renderer can write them as `data-sourcepos`. pulldown-cmark gives a byte range per event, which is as precise but has to be carried through a renderer we would write.

**Extensions we need.** Both: front matter (`---` YAML), `$`/`$$` math, footnotes, tables, task lists, strikethrough. comrak also: GFM extended autolinks; footnotes rendered as GitHub does (numbered by first reference, a footnotes section with back references).

**Isolation.** Both parse any string, so a block range from the buffer can be parsed alone. Rendering the first 2 MB block by block differs from the whole-file render only where blocks depend on each other: reference definitions, footnotes. Both parsers offer a broken-link callback through which a global reference-definition table can answer for definitions outside the block (ADR-0027).

**Rendering hooks.** comrak's `create_formatter!` overrides the HTML for individual node types and falls back to its spec-tested defaults for the rest. We need that for heading ids, image and link rewriting (local images through our scheme, remote ones as placeholders, ADR-0028), footnote numbering across blocks, code blocks (highlighting, Mermaid) and math placeholders.

**Maintenance and license.** comrak 0.56.0 (2026), BSD-2-Clause, maintained (kivikakk), fuzzed upstream; with default features off its dependencies are `caseless`, `memchr-n`, `phf`, `rustc-hash`, `smallvec` and `typed-arena` (MIT or MIT/Apache-2.0), and `finl_unicode`, `(MIT OR Apache-2.0) AND Unicode-DFS-2016`. Unicode-DFS-2016 is the predecessor of the already-approved Unicode-3.0 with the same grant; the user approved it on 2026-10-13, and it is now in `deny.toml` and the JS gate. pulldown-cmark 0.13.4, MIT, maintained.

## Decision

Use **comrak 0.56** (default features off) as the Markdown parser in `crates/markdown`, with these extensions on: table, strikethrough, autolink, tasklist, footnotes, front matter (`---`), `math_dollars`. Rendering goes through a `create_formatter!` formatter of our own that overrides the nodes listed above and keeps comrak's defaults for everything else.

The spec harness (task 3) runs our renderer, before sanitization, against all three suites. The exceptions are listed in `docs/markdown-spec-exceptions.md`.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| pulldown-cmark 0.13 | MIT | 3× faster, but no extended autolinks (19 spec examples), so we would write and maintain a GFM autolinker; no renderer hooks, so we would write the whole HTML renderer to add source positions, placeholders and footnote numbering. Its speed matters only for background indexing. |
| `markdown` (markdown-rs) 1.0 | MIT | mdast with positions and GFM, but slower than both in published benchmarks and not evaluated further; no advantage on any criterion above. |
| Our own parser | | Months of work to reach the spec pass rate both crates already have. |

## Consequences

- First paint and edits never wait for a whole-file parse: ADR-0027 parses blocks around the viewport first and indexes the rest in the background.
- A whole-file parse costs about 0.7 s raw on 20 MiB (≈ 1.0 s in ref-sim terms), as background work. If that ever matters, the block index can switch to a lighter scan without changing the renderer.
- comrak updates can change rendering; the spec harness and the committed test cases (`tests/data/markdown/`) catch regressions. The version is pinned in `Cargo.lock`.
- The GFM tag filter is the sanitizer's job, not the parser's.
