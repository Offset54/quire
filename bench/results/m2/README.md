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
