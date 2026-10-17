# M2 results

Ref-sim runs (ADR-0021) on the dev machine; not reference-machine numbers.

| File | Commit | Notes |
|---|---|---|
| `parser-spike-2026-10-13.json` | | Parser spike (ADR-0026) |
| `refsim-md-2026-10-14-233935.json` | 242d0d0 | First Markdown run; keystroke and typing rows judged raw. Keystroke p95 315 ms from platform stalls (RISKS #12) |
| `refsim-md-2026-10-14-234605.json` | 242d0d0 (bench uncommitted) | Keystroke and typing rows judged on own values (not the rule later decided); fails keystroke p95/max |
| `refsim-txt-2026-10-14-234929.json` | 3110379 | TXT after the M1 refetch fix: 20/21; `txt300.caret.max` failed (End before the index completed scanned the line, fixed in e8faf94) |
| `refsim-md-2026-10-15-002701.json` | 7e73047 | **Disturbed**: fuzz and test builds ran during the run (one long task at open, keystrokes delayed 170–190 ms with 1–4 ms of host time). Not valid |
| `refsim-txt-2026-10-15-002701.json` | 7e73047 | **Disturbed**: test builds ran during part of the run. 21/21 pass, kept for reference only |
| `refsim-md-2026-10-15-003943.json` | 02a5733 | Clean. 10/11: keystroke judged on unstalled keystrokes (ADR-0024 addendum) p95 21.1 ms, max 24.8 ms; typing p99 71.1 ms raw in one run of three (a platform stall; own 17.6 ms) |
| `refsim-txt-2026-10-15-003943.json` | 02a5733 | Clean. 21/21 |
| `refsim-md-2026-10-15-023333.json` | 94a4cfc | Clean. 10/11 with typing judged on own values (ADR-0024 addendum 2). No platform stalls this run, so `md.keystroke.max` 35.1 ms counted: reference-definition keystrokes waited for the O(document) table rebuild because `md_blocks` took the edit from the worker (fixed in 74bbb67). The TXT suite after it hung (fixed with `refsim run --timeout`, fb7c787) |
| `refsim-md-2026-10-15-170811.json` | 30928b7 | Clean, with highlighting, KaTeX and Mermaid. **11/11**: first paint 113 ms (333 effective), keystroke p95 22.7 / max 28.5 ms (33.3), typing p99 7.7 ms own, preview max frame 11.4 ms, no long tasks |
| `refsim-txt-2026-10-15-170611.json` | 30928b7 | Clean. **21/21**; launch-2g 899 ms (own 835) |
| `refsim-m0-2026-10-15-170611.json` | 30928b7 | M0 suite (first run since 2026-10-08): **startup.warm 542 ms fails** (500 effective; 457–480 on 2026-10-08), cold 491 ms passes, scrolling rows pass. Binary 51.4 MB (18.6 MB before the grammars); attribution in progress (CLAUDE.md) |
| `startup-attribution-2026-10-15.json` | ce4b72f | Warm-start attribution, interleaved launch by launch in ref-sim. Pre-highlighting build (94a4cfc) vs main: medians 528 / 524 ms, paired difference −12 ms (main slower in 7 of 16 pairs). Three-way with the bare WebView2 floor and an M1 build (e3209dc): medians 421 / 494 / 513 ms, main − M1 +17 ms paired. The machine drifted within the run (floor 529 → 378 ms), so the M2 cost is at most about 20 ms and inside the noise; the grammars don't explain the 542 ms of 17:06. WebView2 155 (154 until 10-13); its floor (421 ms) is not slower than on 10-08 (449 ms) |
| `refsim-m0-2026-10-15-175725.json` | ce4b72f | M0 suite right after the attribution, machine idle: **11/11**, startup.warm 420 ms (500 effective), cold 441 ms |
| `export-spike-2026-10-15.json` | 4d90871 | Export spike (raw dev machine, not ref-sim): 1 MiB slice HTML 4.6 s / PDF 47.5 s, 1,131 pages; 4 MiB and 20 MiB PDF didn't finish (WebView2 renderer 5.9 / 12.6 GB, stopped). User decision: PDF refused over a 10 MiB page, `krilla` in M6 (ADR-0031) |
| `export-streamed-2026-10-17.json` | 76a3a61 + RISKS #17 | Streamed export (raw dev machine, not ref-sim; ADR-0031 addendum): 1 MiB slice HTML 1.1–3.2 s, PDF 40–53 s, no long task; main-thread gaps ≤ 8 ms in 6 of 7 HTML exports (one had 10 and 12 ms), PDF ≤ 7.7 ms. 20 MiB HTML 35 s (27 s warm, was ~3 min), no long task; gaps over 8 ms only at the very end (8–11 ms, and one 62–65 ms pause after it resolved, no script in the frame) |
| `criterion-markdown-2026-10-16.json` | b705a76 | Criterion (raw): 4 MiB block index 155 ms, whole document 383 ms, keystroke + undo 1.16 ms, one block render 5.5 µs |
| `refsim-md-2026-10-17-001130.json` | b705a76 | **Disturbed** (the machine was in use): first paint 501 / 89.5 / 568 ms with a 50 ms long task in two runs; the other rows pass. Kept for reference |
| `refsim-txt-2026-10-17-001545.json` | b705a76 | **21/21**; save ratio 1.25 (at the limit) |
| `refsim-m0-2026-10-17-002018.json` | b705a76 | **11/11**: startup.warm 428 ms (500 effective), cold 440 ms |
| `refsim-md-2026-10-17-005414.json` | b705a76 | Clean. **11/11**: first paint 84–88 ms (333 effective), keystroke p95 23.3 / max 25.1 ms (33.3), no platform stalls, typing p99 7.4 ms, preview max frame 8.1 / 13.8 ms, no long tasks, +59 MiB |
