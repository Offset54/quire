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
