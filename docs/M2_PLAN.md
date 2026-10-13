# M2 Markdown: task breakdown

Estimates are in focused engineer-days for one engineer, and include tests and docs for each task. They'll be re-estimated from actuals at the M2 report.

## Scope and DoD (§11)

Scope: incremental parsing, the virtualized preview, scroll sync, lazy math and Mermaid, sanitization, and exports.

From the capability matrix (§3.1), v1:
- **View:** source view plus live preview (split or preview-only), outline, CommonMark + GFM (tables, task lists, footnotes, strikethrough, autolinks), YAML front matter, KaTeX math, Mermaid, code highlighting.
- **Edit:** source editing with incremental preview, two-way scroll sync, image paste into an assets folder, table helpers.
- **Save:** untouched regions stay byte-exact (the M1 engine already guarantees this: the source of truth is the text buffer).
- Not in v1: WYSIWYG mode (v2), DOCX export (v2).

DoD:
- At least 99 % of the CommonMark and GFM spec tests pass, with the exceptions listed (in the M2 report and `docs/markdown-spec-exceptions.md`).
- The Markdown row in §4 is met, measured in ref-sim against `bench/budgets.json`: on the 20 MB fixture, first paint of source and preview ≤ 500 ms, and keystroke → preview update ≤ 50 ms.
- The cross-cutting rows hold for Markdown too: typing p95 ≤ 16 ms / p99 ≤ 33 ms in the source view, 60 fps preview scrolling with no frame over 50 ms (platform stalls reported separately, ADR-0024), no UI-thread task over 50 ms.

§6.2 governs the design: the text buffer is the source of truth; re-parse only the blocks an edit touches, widened to the enclosing container; track reference definitions globally; virtualized preview with height estimates and a measurement cache; a source map (block ↔ line range) drives scroll sync and click-to-source; sanitize embedded HTML with an allowlist; block remote images by default with a per-document toggle; render KaTeX and Mermaid lazily, Mermaid in an isolated offscreen context, with the source and an error badge on failure; exports: self-contained HTML and PDF.

Carried into M2 (owed from M1, `PROGRESS.md` known gaps): report the webview tile-bitmap cache to the memory governor (owed before M3); a command-line file that fails to open shows no message; reopening with another encoding has no progress or cancel.

## Effective budgets in ref-sim

ADR-0021 derates `cpu` work by 1.5 and `mixed` work (cold I/O plus CPU) by `max(1.5, D_io)`. `D_io` was 1.62–1.74 in M1; the table uses 1.74.

| §4 row | Budget | Kind | Effective here |
|---|---|---|---|
| MD 20 MB: first paint of source and preview (cold file) | 500 ms | mixed | ≈ 287 ms |
| MD 20 MB: keystroke → preview update | 50 ms | cpu | ≈ 33 ms |
| Typing → visual response in the source view | p95 16 ms, p99 33 ms | cpu | p95 10.7 ms, p99 22 ms |
| Scrolling the preview | 60 fps, no frame over 50 ms | raw | p99 frame ≤ 16.7 ms, max ≤ 50 ms, without platform stalls (ADR-0024) |
| UI-thread long tasks | none over 50 ms | zero | 0 |

Our readings, to be recorded in `bench/budgets.json`:
- **First paint** = file-open command → the first viewport of both panes shows correct content (source rows drawn, and preview blocks rendered for the visible part, with KaTeX/Mermaid/highlighting allowed to arrive later as labeled placeholders, §7 "progressive everything"). Measured on the cold 20 MB fixture in split view.
- **Keystroke → preview update** = keystroke → the frame where the preview shows the edited block re-rendered. Measured at several positions (start, middle, inside a big table, inside a list, editing a reference definition used far away). Math, Mermaid and highlighting inside the edited block may update after that frame, labeled; plain text and structure may not.

## Tasks

| # | Task | Output | Est. (d) | Depends on |
|---|---|---|---|---|
| 1 | Parser spike | `comrak` vs. `pulldown-cmark` (both permissive): CommonMark 0.31.2 and GFM spec pass rates as shipped, throughput on the 20 MB fixture, source positions per block, front matter, footnotes, `$` math, extended autolinks, how a block range can be parsed in isolation; maintenance and license check; **ADR-0026** | 2 | none |
| 2 | Block index and incremental parsing | `crates/markdown`: a top-level block index over the text buffer (line ranges, container kind), built viewport-first and completed in the background; on an edit, re-parse the touched blocks widened to the enclosing container (list, block quote, fenced code, HTML block, table) until block boundaries line up again; a global reference-definition table with dependents, so editing a definition re-renders the blocks that use it; footnote numbering kept consistent; reads through `Snapshot` ranges, never one whole-file buffer (§5.5); property test: incremental result == full parse after random edit sequences; **ADR-0027** | 6 | 1 |
| 3 | Block rendering and spec suite | Per-block HTML with source-map attributes; GFM tables, task lists (read-only checkboxes in v1 unless trivial to make editable), footnotes, strikethrough, autolinks; YAML front matter as a collapsible metadata panel; heading ids (GitHub slug rules) and the outline; spec harness: CommonMark and GFM spec tests fetched into `.cache/`, run against the renderer before sanitization, ≥ 99 % with every exception listed | 4 | 2 |
| 4 | Sanitization and content loading | Allowlist sanitizer in the core for raw HTML (blocks and inline): no scripts, event handlers, `javascript:` URLs, iframes or forms; remote images and other remote resources replaced by visible placeholders until the per-document "load remote content" toggle is on (§8); local images served through a scoped custom URI scheme (rules per decision B below), decoded at display size via the governor; CSP stays strict; hostile-HTML test set; **ADR-0028** | 3 | 3 |
| 5 | Code highlighting | tree-sitter in the core, highlight spans per code block, lazy for visible blocks and cached by content hash; grammar set per decision D below, each grammar's license checked; honest degradation (blocks above a size cap are shown plain, labeled); **ADR-0029** | 3 | 3 |
| 6 | Preview surface | Virtualized DOM block view: height estimates, a measurement cache keyed by (block hash, width, theme), M0's virtual scroll model and custom scrollbar for huge extents; viewport requests with streaming responses over the binary IPC path; placeholders while blocks are pending; patching only changed blocks after an edit; **ADR-0030** (DOM blocks vs. core tiles for the preview) | 5 | 3 |
| 7 | KaTeX and Mermaid | Both bundled locally (no network) and loaded on first use; KaTeX rendered per block when it enters the viewport; Mermaid in a sandboxed offscreen iframe (no same-origin), one queue, results sanitized and cached by source hash; on failure the source with an error badge; licenses of their dependency trees checked by the JS license gate | 4 | 6 |
| 8 | Source view and incremental preview | `.md` files open in the TXT DOM-row view (ADR-0023) with light Markdown decoration (headings, emphasis, code fences, links) from the parse; the edit → re-parse → patch pipeline within the keystroke budget; view modes: split, preview-only, source-only; status bar word count | 4 | 2, 6 |
| 9 | Scroll sync, outline, click-to-source | Two-way scroll sync from the source map (interpolated within blocks, no feedback loops), click in the preview → caret in the source, outline in the left rail with the current section highlighted | 3 | 6, 8 |
| 10 | Editing helpers | Image paste: clipboard image → PNG in the assets folder (decision A), name by date + content hash, dedupe, relative link inserted, all one undo step; table helpers: format and align a table, insert/delete row and column, Tab/Shift+Tab between cells; every helper an undoable transaction that leaves untouched text byte-exact | 4 | 8 |
| 11 | Exports | Self-contained HTML (inline CSS; images as data URIs; KaTeX CSS and fonts embedded; Mermaid as inline SVG; remote images only if the toggle is on) and PDF; engine choice (webview print-to-PDF vs. core via `krilla` from the HTML-free block model) by a short spike; export reports list anything not carried over; **ADR-0031** | 4 | 3, 7 |
| 12 | Corpora, fixtures and fuzzing | Spec corpora fetched by script (license-tracked); committed `tests/data/markdown/` cases (nested containers, lazy continuation, reference definitions, hostile HTML, CRLF, BOM); fuzz targets: parser, incremental-vs-full differential, sanitizer; added to the nightly workflow | 2 | 2, 4 |
| 13 | Benchmarks | Markdown rows in `bench/budgets.json`; in-app scenarios `md-20m` (cold first paint, both panes) and `md-20m-edit` (keystroke → preview update at the positions above, typing latency, preview scrolling steady and fling); `tools/bench/refsim-md.ps1` against the `md` budgets, with ADR-0024 stall reporting; criterion benches for block-index build and incremental re-parse | 3 | 6, 8 |
| 14 | WebDriver UI flows | Open a `.md` from the command line; split view shows both panes; typing updates the preview; scroll sync both ways; click-to-source; remote image placeholder and toggle; image paste; table format; export HTML | 2 | 8–11 |
| 15 | Carried from M1 | Tile-bitmap cache reported to the governor (frontend-reported budget); message for a command-line file that fails to open; progress and cancel for reopening with another encoding | 2 | none |
| 16 | M2 report | `docs/M2_REPORT.md` with DoD evidence; ADRs; PROGRESS, RISKS, ARCHITECTURE updated | 1 | all |
| | **Total** | | **52** | |

With a 20 % contingency, M2 takes about 62 working days of one engineer's time.

## Order

1. Task 1 first, with task 15 alongside (independent).
2. Then 2 and 3; 4 and 5 once rendering exists; 12 alongside 2–4 (the differential fuzz target guards task 2 from the start).
3. Task 6, then 7 and 8; then 9 and 10.
4. Task 11 once rendering and Mermaid/KaTeX are stable.
5. Tasks 13 and 14 last, then the report. Run ref-sim with the machine idle and the disk rested (CLAUDE.md).

## Decisions to make in M2 (ADRs)

- **0026 Markdown parser:** `comrak` vs. `pulldown-cmark`, by spec pass rate, throughput and source positions.
- **0027 Incremental parsing:** block index, container widening, reference definitions and footnotes across blocks.
- **0028 Sanitization and content loading:** HTML allowlist, remote-content toggle, local-image scope.
- **0029 Code highlighting:** tree-sitter integration, grammar set, size cap.
- **0030 Preview surface:** virtualized DOM blocks vs. core-rendered tiles.
- **0031 Exports:** PDF engine for Markdown export.

## Decisions for the user (scope and UX), with recommendations

Recorded here once answered; the executing session should not decide them itself.

- **A. Where pasted images go.** Recommended: an `assets/` folder next to the document (the convention the 20 MB fixture already uses), files named `image-<yyyymmdd>-<hash8>.png`. Alternative: a per-document `<name>.assets/` folder.
- **B. Which local images the preview loads without asking.** Recommended: relative paths inside the document's folder (and its subfolders) load automatically; absolute paths and paths that leave the folder are treated like remote content, behind the same per-document toggle. Alternative: any readable local path loads.
- **C. Default view when a `.md` opens.** Recommended: split (source left, preview right), remembered per user. Alternatives: preview-only, or source-only.
- **D. Highlighting languages in v1.** Recommended (about 20): Bash, C, C++, C#, CSS, Go, HTML, Java, JavaScript, JSON, Kotlin, Markdown, PowerShell, Python, Ruby, Rust, SQL, Swift, TOML, TypeScript, YAML. Others render as plain code, labeled with their language.

## Risks to watch

- **Full-document dependencies vs. viewport-first paint.** Reference definitions and footnotes can be defined anywhere, so the first paint may render a link before its definition is known. Mitigation: render with what is known, and patch the affected blocks when the background block index completes (labeled, never silently wrong for long).
- **Preview height estimates on 20 MB.** Tens of thousands of blocks with estimated heights; the scrollbar can drift as measurements arrive. Mitigation: M0's virtual scroll model with anchor-preserving corrections.
- **Mermaid's size and isolation.** A large bundle with its own DOM needs; an isolated iframe adds a per-diagram round trip. Lazy loading and caching by source hash keep it off the first paint.
- **Platform frame stalls (RISKS #12)** affect every first-paint and scroll row; ADR-0024's reporting applies unchanged.
