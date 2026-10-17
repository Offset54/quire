# M2 Markdown: report

- Date: 2026-10-17 (plan approved 2026-10-13)
- Commits: `c87a070` (plan) … `236c2b8` (last code), then this report, on `main` ([Offset54/quire](https://github.com/Offset54/quire))
- CI: build, tests and gates green on Windows, macOS and Linux, and the WebDriver UI-flow job green on Windows, at `236c2b8`. The fuzz workflow (eight targets, 10 minutes each, under AddressSanitizer and LeakSanitizer) is green at `236c2b8` (run 38999200450), after its runs on 2026-10-16/17 found five more bugs (below).
- **M2 is done.** Both DoD items are met: the CommonMark and GFM spec suites pass at 99.26 % with every exception listed, and the final ref-sim run meets all 11 Markdown rows of §4 (TXT 21/21 and the M0 suite 11/11 alongside). One decision was yours this stretch: PDF export of very large documents (below).

## Definition of Done (§11)

| DoD item | Status | Evidence |
|---|---|---|
| At least 99 % of the CommonMark and GFM spec tests pass, with the exceptions listed | **Met** | 1,342 of 1,352 (99.26 %) through the app's own pipeline (block index, per-block analysis, document-wide tables, block rendering): CommonMark 0.31.2 652/652, GFM spec 661/670, GFM extensions 29/30. The ten exceptions are listed with reasons in [markdown-spec-exceptions.md](markdown-spec-exceptions.md) (nine are spec drift: the GFM spec embeds CommonMark 0.29's emphasis rules). The harness fails on any unlisted failure and on a listed exception that starts passing; rerun 2026-10-17. |
| The Markdown row in §4 is met | **Met** | Ref-sim, 20 MiB fixture, three runs per scenario: first paint 85.1 ms (500 ms budget, 333 ms effective); keystroke → preview update p95 23.3 ms / max 29.6 ms (50 ms, 33.3 ms effective). Table below. |

### §4 Markdown row and the M2 rows in ref-sim

Ref-sim profile (ADR-0021): 4 P-cores, 8 logical CPUs reported, 1280×720 logical at 150 %, `D_cpu` 1.5. "Effective" is the budget divided by the derating. Platform frame stalls (RISKS #12) are reported apart and not charged to us (ADR-0024 and its addenda, your decisions of 2026-10-12 and 2026-10-15).

Final run: `bench/results/m2/refsim-md-2026-10-17-014248.json`, bench build of `6f67702`, machine idle, after a three-minute rest. The code changed after it in ways that don't touch these paths' costs: patched grammar scanners (four comparisons and one `free`), the normalized footnote label of definition blocks, tests and CI; not re-measured.

| Row | Budget | Effective | Result (median of 3 runs; per run) |
|---|---|---|---|
| First paint of source and preview, cold 20 MiB file | 500 ms | 333 ms | **85.1 ms** (82.3, 85.1, 89.1) |
| Keystroke → preview update, p95 / max ¹ | 50 ms | 33.3 ms | **23.3 / 29.6 ms** (p95 22.4 / 23.3 / 23.6; max 30.7 / 29.6 / 26.5) |
| Typing in the source view, p95 / p99 | 16 / 33 ms | 10.7 / 22 ms | 7.2 / 7.5 ms |
| Preview scrolling, steady: p99 / max frame | 16.7 / 50 ms | raw | 7.4 / 8.5 ms |
| Preview scrolling, fling: p99 / max frame | 16.7 / 50 ms | raw | 7.4 / 13.8 ms |
| Long tasks while opening, editing and scrolling | 0 | | 0 |
| Memory after the index completes | 1.5 GB | | +59 MiB |

¹ 100 keystrokes per run at five positions (start, middle, a list, a 1,500-row table, a reference definition), 120 ms apart, timed to the preview's updated frame. Judged on keystrokes the platform didn't stall (ADR-0024 addendum); none stalled in this run (p50 9.5 ms).

Earlier runs, all in [bench/results/m2/README.md](../bench/results/m2/README.md): 2026-10-15 17:08 at `30928b7` 11/11 (first paint 113 ms); 2026-10-17 00:54 at `b705a76` 11/11 (first paint 84–88 ms, no platform stall in 100 keystrokes). One run, 2026-10-17 00:11, while the machine was in use, had first paints of 501 and 568 ms in two of three runs and one 50 ms long task each; it is kept and marked disturbed. The 40-minute-later rerun on the same build was clean.

The other suites on the final build: TXT 21/21 (`refsim-txt-2026-10-17-014646.json`) and M0 11/11 (`refsim-m0-2026-10-17-015022.json`). Warm start passed at 488 ms (runs 468–599) of 500 ms effective; earlier the same day it was 420 and 428 ms. The margin is thin and moves with the machine's state, so it stays on the watch list (RISKS #15).

## Decisions taken in M2

1. **Plan decisions A–D (2026-10-13):** pasted images in `assets/` next to the document, deduplicated; any readable local image loads, remote content behind a per-document toggle ([ADR-0028](adr/0028-markdown-sanitization-and-content-loading.md) records the trade-off); Obsidian-style source and reading views with Ctrl+E and split view; 21 highlighted languages.
2. **Unlicense allowed** for Mermaid's `robust-predicates`; Mermaid pinned at 11.17.2 to keep EPL-2.0 `elkjs` out ([ADR-0032](adr/0032-unlicense-allowed.md), 2026-10-15).
3. **Keystroke and typing rows judged on frames the platform delivered** (ADR-0024 addenda 1 and 2, 2026-10-15).
4. **PDF export of very large documents (2026-10-16):** keep WebView2's printer for M2, refuse a page over 10 MiB with a message pointing to HTML export, and write PDFs with `krilla` in M6 as the brief plans ([ADR-0031](adr/0031-markdown-export.md)). The spike behind it: the notes sample (48 KB) prints in under a second; a 1 MiB slice of the 20 MiB fixture in 47.5 s (1,131 pages); 4 MiB and 20 MiB never finished, the WebView2 renderer reaching 5.9 and 12.6 GB.

## Scope checklist (§11 M2)

| Item | Where |
|---|---|
| Incremental parsing | `crates/markdown`: block index built viewport-first and completed in the background, edits re-parse the touched blocks widened until boundaries line up, document-wide reference and footnote tables ([ADR-0026](adr/0026-markdown-parser.md) comrak, [ADR-0027](adr/0027-markdown-incremental-parsing.md)) |
| Virtualized preview | Byte-anchored DOM blocks in host windows, render cache, large tables and code as pieces, outline rail ([ADR-0030](adr/0030-markdown-preview-surface.md)) |
| Scroll sync | Two-way, interpolated within blocks; click-to-source |
| Lazy math and Mermaid | KaTeX in the page in 4 ms slices; Mermaid in a sandboxed frame from the `quire-frame` scheme, SVG shown as an image; errors keep the source with a badge ([ADR-0033](adr/0033-math-and-diagrams.md)) |
| Sanitization | Allowlist sanitizer in the core for raw HTML and, since this stretch, for Markdown links too; images through the `quire-img` scheme ([ADR-0028](adr/0028-markdown-sanitization-and-content-loading.md)) |
| Exports | Self-contained HTML and PDF with a report of what wasn't carried over ([ADR-0031](adr/0031-markdown-export.md)) |
| Also | Code highlighting with tree-sitter ([ADR-0029](adr/0029-code-highlighting.md)); table helpers and image paste; carried M1 gaps (task 15) |

## Tests

- Rust: `quire-markdown` unit tests with two proptests (incremental index = full parse, 100,000 cases at task 2; document state), the spec harness, the hostile-HTML suite (`tests/data/markdown/hostile/`, now with Markdown links) checked by a tokenizer written apart from the sanitizer; `quire-highlight` tests over every grammar; the host's export test.
- Criterion (`crates/markdown/benches/markdown.rs`, raw): 4 MiB block index 155 ms, whole document with analysis 383 ms, keystroke and undo 1.16 ms, one block render 5.5 µs.
- Frontend: 36 vitest tests.
- UI flows (WebDriver): 15 Markdown flows (open, split view, images and the remote toggle, typing, scroll sync, reading view, save, highlighting, math and diagrams, outline, large tables, table helpers, image paste, export), 27 in all with TXT; all pass locally on the final build.
- Fuzzing: eight targets, four new in M2 (`md_scan`, `md_incremental`, `md_sanitize`, `highlight` over every grammar), nightly on CI. Local runs on 2026-10-15/16: md_scan 201k inputs, md_incremental 116k + 76k, md_sanitize 33k, highlight 347k, search 108k.
- In-app scenarios `md-20m` and `md-20m-edit`, run by `tools/bench/refsim-md.ps1`.

## Problems found and fixed in this stretch (2026-10-15 to 17)

1. **Warm-start miss attributed.** The M0 suite failed warm start at 542 ms (500 effective) after the grammars grew the binary to 51 MB. Launches interleaved in ref-sim put the cost of M2 at about −12 to +17 ms, inside the machine's drift (the bare WebView2 floor itself moved 529 → 378 ms in one run); the M0 suite then passed (warm 420 and 428 ms). RISKS #15 updated.
2. **A byte order mark past the document's start** (`\u{feff}> q`) parsed differently in a stretch of its own, because comrak skips a leading BOM (fuzz: md_scan, md_incremental). Fixed with U+2060 in its place for parsing (ADR-0027 addendum).
3. **Markdown links skipped the link allowlist** (fuzz: md_sanitize): `[x](ms-msdt:…)`, `ms-settings:` and unknown schemes stayed live links, against ADR-0028; they also lacked `rel="noopener noreferrer"`. The scheme check also looked only at a scheme's first 16 characters. Fixed (ADR-0028 addendum).
4. **Front matter moved by an edit** (fuzz on CI: md_incremental): text typed above front matter left the old block in place. Fixed.
5. **Search missed or shortened matches longer than the chunk overlap** (fuzz on CI: search, an M1 target): a match over 64 KiB whose beginning matched nothing in the window was missed, and a greedy match that backtracked inside the window came out short. Patterns whose longest match fits the overlap (every literal search) keep the 64 KiB window; others search up to the documented 16 MiB cap past each chunk, which costs them up to twice the reading.
6. **The flaky decode-progress UI flow** raced startup since the binary grew; it now checks progress and Cancel through an in-app open as well.
7. **The export flow failed on the CI runner**: the hidden PDF window got different WebView2 browser arguments from the main window under WebDriver (0x8007139F). It now gets the same ones.
8. **The table-helper UI flow** raced its own saves (an edit during a save is refused "busy"); it waits for the view first now.
9. **A crash in the Markdown grammar's C scanner** (fuzz on CI: highlight, symbolized once the workflow had a symbolizer): it passed Unicode code points to `isdigit()`, which glibc answers from a 256-entry table, a SEGV reachable from any document with a `markdown` code block. The Bash scanner had the same calls; the other 15 external scanners have no such calls. Both grammars are vendored with the four calls fixed ([ADR-0034](adr/0034-patched-grammars.md)); upstream is still unfixed.
10. **The highlight target's leak** (fuzz on CI, symbolized): the SQL grammar's scanner dropped its dollar-quote tag in `deserialize` without freeing it, a small leak per highlighted SQL block. Upstream has the fix, unreleased; vendored with it (ADR-0034).
11. **Footnote labels depended on the stretch** (fuzz on CI: md_incremental): comrak renames a definition after a reference in the same parse, so a block's kind could differ between the incremental and the full parse. The kind now holds the normalized label.
12. **The outline UI flow** read the heading count before it had followed an edit (401 instead of 400 on the runner); it waits for the converged count now.
13. **New advisories for `zip`** (RUSTSEC-2026-0367, -0368, -0369, about extraction), reached only through the fixture generator: upgraded to 9.0.4, fixtures byte-identical.

## Deviations from the plan

1. **Mermaid in exports is an SVG `<img>`**, not inline `<svg>` as M2_PLAN said: inline SVG from a hostile diagram is live markup in the exported page; as an image it renders the same and stays inert (ADR-0031).
2. **PDF export has a size limit** until M6 (decision 4).
3. **Fuzz findings outside M2**: the search fix (item 5) changes M1 code.
4. **Two third-party crates are vendored** with a patch (item 9, ADR-0034), until upstream fixes them.
5. **Estimate.** M2 was planned at 53 engineer-days (64 with contingency). It took five calendar days of agent work (2026-10-13 to 17); engineer-day estimates don't translate to that, so the plan's numbers stay as the yardstick for scope, not schedule. Task 11 (exports) grew from 4 days to include the spike and a size limit; task 12 found seven bugs, among them one in M1 code and one in a third-party C scanner.

## Known gaps

- **Export: residual main-thread gaps.** The export blocked the window until 2026-10-17 (RISKS #17); it is streamed now (`b913d4b`, `eba2aa8`, see the addendum). Left: a few 8–12 ms gaps, mostly at the end of a 20 MiB export, and one 62–65 ms pause with no script in the frame (likely GC). PDF pages over 10 MiB are refused until `krilla` replaces the printer in M6.
- **Fonts on first use** (RISKS #13): a document that draws before the font worker has finished, or text in a script the warm-up doesn't sample (Arabic, Thai, Devanagari, CJK ideographs), still pays Windows font fallback on the UI thread.
- PDF export works on Windows only so far; macOS and Linux need their own print path (M7).
- Huge lists, block quotes and HTML blocks are one preview entry each (RISKS #14); the C parsers stay a fuzzing target (RISKS #16).
- The Microsoft IMEs still haven't been checked by hand (carried from M1).

## Addendum 2026-10-17: export streaming and the open-time task

Both measured with `tests/ui/export-spike.test.mjs` on a release build, raw dev machine (not ref-sim). The spike now has a MessageChannel gap probe (a gap is how long other work held the main thread), Long Animation Frame attribution, a 3 s idle baseline and an open inside the probe.

**Exports** (RISKS #17 mitigated, `b913d4b` and `eba2aa8`, ADR-0031 addendum, `bench/results/m2/export-streamed-2026-10-17.json`). The core renders the page block by block on its own thread, a Web Worker renders the math, and the page relays pieces to the host, with progress and Cancel.

| | Before (2026-10-15) | After |
|---|---|---|
| 1 MiB slice, HTML | 4.6 s, long tasks up to 2.6 s | 1.1–3.2 s, no long task; gaps ≤ 8 ms in 6 of 7 exports (one had 10 and 12 ms) |
| 1 MiB slice, PDF | 47.5 s, long tasks up to 3.2 s | 39.6–52.8 s, 1,131 pages, no long task, max gap 6.7–7.7 ms |
| 20 MiB fixture, HTML | ~3 min | 34.8 s first, 27.1 s warm; no long task; gaps over 8 ms only in the last half second (8–11 ms, and one 62–65 ms pause with no script in the frame) |

**The open-time task** (RISKS #13, ADR-0030 addendum, `bench/results/m2/open-fonts-2026-10-17.json`). Opening the 1 MiB slice from the command line showed a 64–107 ms task in 2 of 4 runs while measuring the exports, with no export running. That breaks §4's "no UI-thread task over 50 ms" (and §2's 8 ms). It was the source view's first draw paying Windows font fallback (30–51 ms of forced layout in a requestAnimationFrame callback, mapped through the source map), because a command-line file drew before the font warm-up had finished. The warm-up's own steps were 7–35 ms. A worker now loads the faces off the UI thread. It starts at script start when a command-line file is opening and after the first frame otherwise: starting it at script start always cost warm start a median 7.8 ms over 16 interleaved ref-sim pairs. After the fix: no long task or long frame in 6 of 6 opens, and no gap over 8 ms in the 3 s after (max 5.3–6.8 ms). The warm-up steps take 0–5 ms. An open through the app inside the probe stayed at or under 7.4 ms in 5 of 6 runs; one had 9 and 13 ms gaps at the end of the window, unattributed. UI flows: 28 of 28 pass.
