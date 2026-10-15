# 0029. Code highlighting: tree-sitter in the core, lazy per shown block

- Status: Accepted
- Date: 2026-10-15

## Context

§3.1 lists code highlighting for Markdown in v1; §5 names tree-sitter for it. §2: degrade honestly above a size; §7: progressive everything (highlighting may arrive after the first paint, labeled). The §4 Markdown rows (first paint ≤ 500 ms, keystroke → preview ≤ 50 ms) must hold with it. User decision D (2026-10-13): about 20 languages (Bash, C, C++, C#, CSS, Go, HTML, Java, JavaScript, JSON, Kotlin, Markdown, PowerShell, Python, Ruby, Rust, SQL, Swift, TOML, TypeScript, YAML); others render as plain code, labeled with their language.

## Decision

**A crate of its own, `crates/highlight` (`quire-highlight`)**: tree-sitter 0.27 with `tree-sitter-highlight`, one grammar crate per language of decision D, and the highlight (and, where shipped, injection and locals) queries each crate exports. C++ and TypeScript/TSX extend C's and JavaScript's queries, as their upstream `tree-sitter.json` files do. A fence's info string's first word picks the grammar, by name or common alias (`rs`, `py`, `ts`, `sh`, `ps1`, `yml`, `c++`, `cs`, `kt`, …), case-insensitively. Injections are followed for grammars we ship (JavaScript and CSS inside HTML, Rust macros, code fences inside Markdown and Markdown's inline grammar). Configurations are built once per grammar on first use.

**Output**: the code escaped, with `<span class="hl-…">` around captures, from a fixed list of capture names (`keyword`, `string`, `comment`, `function`, `type`, …; a longer capture takes the closest listed one). The classes come only from our generator: the sanitizer never lets a document's own `class` through (ADR-0028). One palette per theme in `styles.css`.

**Markup**: the renderer writes `<pre data-sourcepos data-lang="…" [data-hl="…"]><code class="language-…">`. `data-hl` says why code isn't highlighted: `none` (no grammar for the language: plain, labeled with the language, decision D), `large` (over the cap: labeled "too large to highlight"), `pending` (not yet). The language shows as a label in the block's corner. Math blocks stay comrak's (task 7).

**Size cap** (§2): code over 256 KiB, or with a line over 16 KiB, renders plain and labeled. Blocks the preview shows as pieces (fenced code over 200 lines, ADR-0030) are highlighted piece by piece, so a construct spanning two pieces (a long string or comment) can be colored wrongly at the cut; the text is always right.

**Off the preview's thread, cached** (`crates/highlight/src/service.rs`): highlighting runs at 0.8–3.3 MB/s and building a grammar's configuration on first use takes up to 357 ms (Swift; C++ 88 ms, Kotlin 65 ms), so the preview never does either while it holds a document's state. A rendering takes highlighted code from a cache keyed by (language, content) hash (8 MiB of HTML, least recently used out); on a miss it renders the code plain (`data-hl="pending"`) and requests it from one background thread, which works newest request first (what is on screen now) and calls the registered listeners when results are ready. The document's worker then renders the shown entries that were waiting again, in passes of 8 ms (now cache hits), and pushes them as a correction window: new revisions of the same block ids, which the frontend swaps in place (ADR-0030).

**An edited block doesn't flash plain**: in the keystroke's own window (`Highlight::New`), code up to 4 KiB whose grammar is ready is highlighted inline (at most a few milliseconds by the measurements below); larger code shows plain for the moment until the background result arrives. Exports highlight everything inline (they wait anyway).

## Measurements

`cargo run --release -p quire-highlight --example hlbench` on the dev machine (raw, not ref-sim), each language's sample repeated to about 200 lines (2–14 KiB) and to the 256 KiB cap, median of 15:

| | 200 lines | 256 KiB | first use (config) |
|---|---|---|---|
| fastest | TOML 0.9 ms, Go 1.3 ms | C 81 ms (3.3 MB/s) | under 1 ms (CSS, JSON, TOML) |
| slowest | Rust 9.9 ms, TSX 9.5 ms, PowerShell 8.9 ms | YAML 318 ms, Markdown 293 ms (0.8–0.9 MB/s) | Swift 357 ms, C++ 88 ms, Kotlin 65 ms, C# 50 ms |

Inline highlighting of an edited block is therefore capped at 4 KiB: 1–4 ms for most grammars on this machine.

## Alternatives

- *Highlighting in the frontend* (a JS highlighter such as highlight.js or Shiki): no C in the core, but regex grammars are less exact, the work lands on the UI thread (§2: no UI task over 8 ms) or a worker with another copy of every block, and TXT's later highlighting (§3.1, v1.x) wants the grammars in the core anyway.
- *comrak's `syntect` adapter*: Sublime grammars through `onig` or `fancy-regex` (not measured here); tree-sitter is what §5 names, and its parsers serve TXT's later highlighting too.
- *Highlight at render time, on the preview's worker*: the first version of this ADR. The measurements above ruled it out: a single block at the cap, or a grammar's first use, would hold the document's state for 100–360 ms, delaying the next keystroke's preview by as much.

## Consequences

- C code in the core: 21 grammars, parsers generated by tree-sitter, with hand-written external scanners in all but C, Go, Java and JSON. A malformed or hostile code block reaches them. Mitigations: the size cap, highlighting only what is shown, its own thread, and a fuzz target over every grammar (task 12).
- Licenses: every grammar crate, `tree-sitter`, `tree-sitter-highlight` and `tree-sitter-language` are MIT (`cargo deny check`). Several grammar crates ship no LICENSE file in the package (C++, HTML, Java, JSON, Markdown, Ruby, SQL, TOML, TypeScript); their repositories carry the MIT text. The third-party notices file (M7) must take those from the repositories.
- Kotlin uses `tree-sitter-kotlin-sg` (ast-grep's fork of the community grammar) because `tree-sitter-kotlin-ng` ships no highlight query; SQL uses `tree-sitter-sequel` (DerekStride's grammar), the maintained SQL grammar on crates.io.
- Build: the grammars add C compilation to a clean build and size to the binary (recorded with the M2 ref-sim run in `bench/results/m2/README.md`).
