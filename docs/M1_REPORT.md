# M1 TXT engine: report

- Date: 2026-10-09
- Commits: `d30c543` (task 1) … HEAD on `main` ([Offset54/quire](https://github.com/Offset54/quire))
- CI: build, tests and gates green on Windows, macOS and Linux; the WebDriver UI-flow job green on Windows (since 2026-10-12); nightly fuzzing green.
- **One decision is yours before M1 can close: how to treat platform frame stalls (and, smaller, how to measure the save ratio).** Everything else is done.

## Definition of Done (§11)

| DoD item | Status | Evidence |
|---|---|---|
| Every TXT row in §4 is met, measured in ref-sim | **Not yet: waiting on your decision.** Run 1 met 19 of 21 budgets; both misses were fixed and pass in run 2. Run 2 failed 4 rows, 3 of them on frames the platform didn't present (RISKS #12), 1 (save ratio) on disk noise around our own 1.25× threshold. | Two full ref-sim runs, table below; `bench/results/m1/refsim-txt-*.json` |
| Property and fuzz tests pass | **Met** | Proptests (piece tree vs. model, undo as exact inverse, journal replay, chunked vs. whole search) in CI. Four fuzz targets ran 10 minutes each: encoding 662,665 inputs, piece_tree 79,417, journal 5,940,193, search 97,676; the encoding target found one bug, fixed (below). Nightly workflow green. |
| Killing the process mid-edit loses at most one second | **Met** | Kill test: a child process killed mid-typing lost 0 of 953 keystrokes (journal records are written at once; fsync at most every second, so a power cut can lose up to one second). UI flow "a killed app offers to restore its unsaved edits" passes. |
| Untouched content survives save byte-for-byte | **Met** | Byte-exact round trips across 11 cases (every supported encoding, BOMs, CRLF/LF/CR, invalid UTF-8); legacy files copy untouched ranges raw ([ADR-0009](adr/0009-encodings-and-transcoding.md)); the save UI flow checks CRLF endings on disk. |

Carried into M1 from M0:

| Item | Status | Evidence |
|---|---|---|
| Open a command-line file in parallel with WebView2 startup (ADR-0022) | **Done** | `launch-2g`, process creation → rows on screen, cold: 937–1,068 ms (run 1), 1,225–1,672 ms (run 2, platform stalls) against the cold startup budget of 1.7 s. A non-UTF-8 file reports decode progress through this path too. |
| WebDriver UI-flow tests (ADR-0020) | **Done** | 9 of 9 flows pass locally and in CI. The runner's WebView2 (153, elevated) ignored msedgedriver's switches, so no DevTools port opened; under tauri-driver the app now applies them itself ([ADR-0020](adr/0020-benchmark-and-test-harness.md) addendum). |

### §4 TXT rows in ref-sim

Ref-sim profile (ADR-0021): 4 P-cores, 8 logical CPUs reported, 1280×720 logical at 150 %, `D_cpu` 1.5, `D_io` measured (1.66 / 1.62), median of three cold runs. "Effective" is the budget divided by the derating. Run 1 at `ffa3e55`, run 2 at `51e0a14` (after the save and long-line fixes).

| Row | Budget | Effective | Run 1 | Run 2 |
|---|---|---|---|---|
| 2 GB first paint | 300 ms | 180 / 186 ms | 149 ms | **571 ms** ✗ ¹ |
| Full line index (background) | 4 s | 2.4 / 2.5 s | 0.76 s | 0.60 s |
| Jump before indexing (approximate), max | 300 ms | 180 / 186 ms | 97 ms | 130 ms |
| Jump, indexed, p95 / max | 100 ms | 66.7 ms | 20 / 21 ms | 19 / 24 ms |
| Literal search, full pass | 3 s | 1.80 / 1.86 s | 1.14 s | 0.38 s |
| Insert at any offset, p95 / max | 5 ms | 3.3 ms | 0.06 / 0.37 ms | 0.04 / 0.11 ms |
| Typing → visual response, p95 / p99 | 16 / 33 ms | 10.7 / 22 ms | 8.1 / 8.4 ms | 9.4 / 10.1 ms |
| Scrolling, steady: p99 / max frame | 16.7 / 50 ms | raw | 7.3 / 13.5 ms | 8.6 / **244 ms** ✗ ¹ |
| Scrolling, fling: p99 / max frame | 16.7 / 50 ms | raw | 7.2 / 8.2 ms | 8.5 / 8.6 ms |
| Long tasks during typing and scrolling | 0 | | 0 | 0 |
| Save after a small edit, 2 GB | ≤ 1.25 × raw write ² | | **1.89 ×** ✗ | **1.255 ×** ✗ ³ |
| Memory beyond the mapped source | 1.5 GB | | 9 MiB | 8 MiB |
| 300 MB single line: first paint | 300 ms | 180 / 186 ms | **189 ms** ✗ | 151 ms |
| 300 MB: caret movement, max | no hang ² | 66.7 ms | 17 ms | 34 ms |
| 300 MB: wrap toggle, max | no hang ² | 66.7 ms | 28 ms | 30 ms |
| 300 MB wrapped: scroll max frame | 50 ms | raw | 7.3 ms | **341 ms** ✗ ¹ |

¹ Platform frame stalls (next section). In these runs the renderer was idle and the GPU process was blocked in DXGI Present.
² Our interpretations, recorded in M1_PLAN and `bench/budgets.json`: §4 says "sequential write speed" for save and "no hang" for the 300 MB line.
³ Run 1 failed because the save read, indexed and wrote in series; that was fixed (writes now overlap reads). Run 2's 1.255 × is noise around the threshold; see "Save ratio" below.

## Decision needed: platform frame stalls (RISKS #12)

**What happens.** On this machine (Arc 140V, 144 Hz panel with a 48 Hz minimum), once nothing on screen changes for about 0.6 s, the next frames are delayed 90–340 ms (once 500 ms). It hits whatever starts after a pause: the first paint, scrolling, a jump.

**It isn't our code.** A Chromium trace of a stall shows the GPU process blocked in `DXGISwapChainImageBacking::Present` (334 ms) while our renderer was idle (long-animation-frame `blockingDuration` 0). Plain Microsoft Edge with a trivial page and no quire code does the same (`bench/results/m1/edge-idle-probe-2026-10-09.json`, `tools/bench/edge-idle-probe.mjs`). Scrolling after 0.7 s idle stalls 280–338 ms in 5 of 5 runs; after 1–2 s, 89–196 ms. Run 1 likely escaped because other windows were animating while it ran.

**What would remove it, and what that costs.** A 1 px compositor animation that keeps frames flowing removed every stall in Edge (worst frame 9.9 ms). It also burned 42–48 % of one CPU core while idle, against 1.5–7 % without. A slower pulse (one change every 400 ms) did not help (stalls of 250–509 ms remained).

**Options:**

1. **Report and measure around it (recommended).** Keep every budget as it is. Treat frames where the platform didn't present (no main-thread work, Present blocked) as platform stalls: report them separately, with the Edge baseline as evidence, and judge the rows on our own frame work (which meets them in both runs). Cost: none in the product. Risk: users on similar laptops will see a 0.1–0.3 s hitch when they start scrolling after a pause, in quire as in Edge.
2. **Ship a keep-alive while the window is focused.** It meets the rows outright on this machine. Cost: about half a core continuously while a document is open and focused, so roughly the battery drain of a playing video. I don't recommend it.
3. **Find the switch first.** You change a display power setting on this machine (Intel Graphics Command Center: Panel Self Refresh / Display Power Saving, or Windows' dynamic refresh rate), and I rerun the probe and the suite to confirm the cause. I can't change system settings myself. This pins down whether the reference-class 2021 laptops would see it at all.

Options 1 and 3 combine: decide 1 now, and do 3 when convenient.

## Smaller decision: how to measure the save ratio

§4 says saving runs at "sequential write speed"; we read that as ≤ 1.25 × a raw sequential write of the same bytes on the same volume, measured right after the save. The save itself now runs at 0.90–1.05 × a raw write when measured in the core (`savebench`, three runs). In the app the comparison is dominated by the SSD: four back-to-back raw writes of 2 GiB took 1.0–2.1 s, and in-app ratios ranged 1.07–1.99 ×.

Options: (a) keep the rule and accept that the row passes or fails by chance on this SSD; (b) **use the mean of raw writes taken right before and right after each save, over five runs** (recommended: same threshold, less noise); (c) judge the row on the core measurement.

## Also for you (no decision needed)

- **Microsoft IMEs.** The UI flow replays the Korean IME's composition events, but no one has typed with the real Korean, Japanese and Chinese IMEs, since screen control is off. Ten minutes of manual checking on your side would close this for M1 (risk #4 stays open for M4's canvas surfaces).
- **Legacy files take seconds to open.** A 2 GB Shift_JIS or CP949 file decodes at 350–560 MB/s cold, so about 4–6 s with a progress banner and Cancel before the first paint (UTF-8 is ≈ 150 ms). §4 has no row for it. Progressive decoding (show the first segments at once) is a follow-up if you want it.

## Scope checklist (§11 M1)

| Item | Where |
|---|---|
| Piece tree, mapped and chunked sources, background line index | `crates/text`: `tree`, `buffer`, `source`, `index` ([ADR-0007](adr/0007-source-access.md), [ADR-0008](adr/0008-text-buffer-piece-tree.md)) |
| Encodings | `encoding`, `transcode` ([ADR-0009](adr/0009-encodings-and-transcoding.md)); decode progress and cancel |
| Viewer and editor: multi-cursor, search/replace | `document`, `history`, `search`, `replace`, `layout`, `view`; frontend `src/txt/` ([ADR-0023](adr/0023-txt-surface-dom-rows.md)) |
| Atomic save with the mapped-file sequence | `file` (two-phase, §5.8 steps), overlapped writes |
| Journal recovery and external change detection | `journal`, `watch`; recovery offer on the welcome screen; reload / keep / compare banners |

## Tests

- Rust: 74 unit tests in `quire-text` (with proptests), plus `quire-core` and the host; criterion benches for the scheduler and the text engine.
- Frontend: 15 vitest tests.
- UI flows (WebDriver, 9): open from the command line, typing/undo/redo, Korean IME composition, a typing burst faster than the core, go to line, find and replace-all, save keeps CRLF, kill and restore, large non-UTF-8 file with decode progress and Cancel.
- Fuzzing: 4 targets with seeds and a regression input; nightly.
- In-app scenarios: `txt-2g`, `txt-2g-search`, `txt-save`, `txt-300m`, `launch-2g`, run by `tools/bench/refsim-txt.ps1`.

## Problems found and fixed in M1 (this stretch)

1. **Quadratic `chars()`.** It re-validated the rest of each UTF-8 run per character, so wrapping, display text and column lookups were quadratic in row length. `view::build` for 175 rows: 50 ms → 1.1 ms unwrapped, over a second → 0.4 ms wrapped.
2. **Re-wrapping per row.** `rows_around` re-read and re-wrapped a whole 64 KiB segment for every row; now once per segment.
3. **Typing latency.** A round trip per keystroke (host 2 ms, IPC ≈ 5.5 ms) plus a frame wait gave 16 ms p50 / 21 ms p95 unpinned. Fixes: local echo with a strict "certain to match" rule (checked against the core in the bench: 0 mismatches in 1,800 keystrokes over the six ref-sim runs), and row keys that don't change for rows an edit didn't touch. Ref-sim p95 8.1–9.4 ms.
4. **Lost go-to-line.** A fetch issued while another was in flight was dropped and replaced by a refetch of the old position. Fetches now queue latest-wins, and jumps can't be overridden by scroll refetches.
5. **Serial save.** Read, index and write ran in series; writes now go to a thread with recycled blocks, and the raw copy prefetches.
6. **Cold scans of long lines.** Before the index reaches a long line, finding its end scanned it in 64 KiB sequential reads (130–140 ms of the 300 MB first paint). Now prefetched and split across four threads (first view 85 ms).
7. **UTF-16 segment cut** (found by fuzzing): a cut after a run of high surrogates could split a valid pair.
8. **macOS and Linux CI** had been red since task 11 (an import used only on Windows).
9. **No decode progress or cancel** when opening a large non-UTF-8 file, though the plan listed a progress banner.
10. **WebDriver on the CI runner.** The runner's WebView2 started without msedgedriver's switches (no `--remote-debugging-port`), so every session timed out. A process probe in the runner script showed it; the app now applies the switches under tauri-driver. The first flow also raced the welcome screen on the slower runner and now waits for the file's rows.

## Deviations from the plan

1. **Treap instead of a B-tree** for the piece tree ([ADR-0008](adr/0008-text-buffer-piece-tree.md)): the same asymptotics, simpler to make persistent; the measured costs leave two orders of magnitude to the budgets.
2. **Task 10 spike** wasn't run as a separate prototype; ADR-0023 rests on the M0 tile measurements and on end-to-end measurements of the DOM surface.
3. **Local echo** is new: the frontend predicts a narrow class of edits (ADR-0023).
4. **Estimate.** M1 was planned at 44.5 engineer-days plus contingency; task 14 (benchmarks) took far longer than its 3 days because of the performance work it uncovered.

## Known gaps

- A command-line file that fails to open (not cancelled) shows no message on the welcome screen.
- Reopening with another encoding decodes without progress or cancel.
- The add buffer isn't compacted after a save ([ADR-0008](adr/0008-text-buffer-piece-tree.md)).
- Still owed before M3: report the webview tile-bitmap cache to the memory governor.
