# Progress

| | |
|---|---|
| Phase | M2 (Markdown) done, report written ([M2_REPORT](M2_REPORT.md)) |
| Product name | Codename `quire` (bundle id `dev.quire.app`) |
| Last updated | 2026-10-17 |

## Milestones

| Milestone | Status | DoD |
|---|---|---|
| M0 Foundations | Done ([report](M0_REPORT.md)) | All 3 met; startup under the revised budget (ADR-0022) |
| M1 TXT engine | Done ([report](M1_REPORT.md), [plan](M1_PLAN.md)) | All 4 met. Ref-sim run 6 (2026-10-13): 21 of 21 TXT budgets, platform frame stalls reported separately (ADR-0024) |
| M2 Markdown | Done ([report](M2_REPORT.md), [plan](M2_PLAN.md)) | Both met: spec suite 99.26 % with the exceptions listed; ref-sim 2026-10-17: 11 of 11 Markdown rows (plus TXT 21/21, M0 11/11) |
| M3 PDF | Not started | n/a |
| M4 DOCX | Not started | n/a |
| M5 PPTX | Not started | n/a |
| M6 PPT, export and print | Not started | n/a |
| M7 Hardening and release | Not started | n/a |

## M2 tasks

| # | Task | Status |
|---|---|---|
| 1 | Parser spike | Done: comrak 0.56 ([ADR-0026](adr/0026-markdown-parser.md)); CommonMark 0.31.2 100 %, GFM spec 98.5 %, extensions 93 % as shipped; 20 MiB parse 693 ms raw (background), one block 2 µs |
| 2 | Block index and incremental parsing | Done: `crates/markdown` index, analysis, tables ([ADR-0027](adr/0027-markdown-incremental-parsing.md)); property test 100,000 cases; 20 MiB index 593 ms raw (background), edit 0.1–0.6 ms |
| 3 | Block rendering and spec suite | Done: per-block rendering through comrak's formatter; spec harness 1,342/1,352 (99.26 %), exceptions in [markdown-spec-exceptions.md](markdown-spec-exceptions.md) |
| 4 | Sanitization and content loading | Done ([ADR-0028](adr/0028-markdown-sanitization-and-content-loading.md)): allowlist sanitizer in rendering, one image policy, `quire-img` scheme (display-size decode, remote images behind the per-document toggle), hostile-HTML set |
| 5 | Code highlighting | Done ([ADR-0029](adr/0029-code-highlighting.md)): `crates/highlight`, tree-sitter 0.27 with the 21 grammars of decision D (all MIT); a background thread with a cache by content hash, shown blocks upgraded after the paint, small edited blocks inline; 256 KiB / 16 KiB-line cap; unknown languages plain and labeled. Measured raw: 0.9–9.9 ms per 200-line block, first use of a grammar up to 357 ms (Swift), off the preview's thread. Binary +33 MB (RISKS #15) |
| 6 | Preview surface | Done ([ADR-0030](adr/0030-markdown-preview-surface.md)): byte-anchored DOM blocks in host windows, render cache with dependency signatures, provisional windows then corrections, progressive placement, large tables and code as pieces, font warm-up. Ref-sim (clean run 2026-10-15): first paint 73–90 ms (333 ms effective), preview scrolling max frame ≤ 9.2 ms, no long tasks; 10 of 11 md rows pass, typing p99 fails in one run of three on a platform stall |
| 7 | KaTeX and Mermaid | Done ([ADR-0033](adr/0033-math-and-diagrams.md), [ADR-0032](adr/0032-unlicense-allowed.md)): KaTeX 0.18.11 lazy, 4 ms slices; Mermaid 11.17.2 in a sandboxed frame served by the `quire-frame` scheme, SVG shown as an image; errors keep the source with a badge. User decision 2026-10-15: Unlicense allowed (Mermaid's d3), EPL-2.0 kept out |
| 8 | Source view and incremental preview | Mostly done: `.md` in the TXT view with decoration, view modes (Ctrl+E, Ctrl+\\, remembered), keystroke → provisional window → corrections, word count. Keystroke → preview in ref-sim: p50 11–15 ms, unstalled p95 20–22 ms / max 24–27 ms; 9–15 % of keystrokes (after a pause) hit platform stalls of 150–375 ms (reported, not judged: ADR-0024 addendum) |
| 9 | Scroll sync, outline, click-to-source | Done: two-way scroll sync, click-to-source, outline rail (virtualized, current section, Ctrl+Shift+O remembered; `md_outline`) |
| 10 | Editing helpers | Done: table helpers (format/align by display width, insert/delete row and column, Tab/Shift+Tab, one undo step each through the engine's new `Replace` action, untouched bytes exact); image paste into `assets/image-<yyyymmdd>-<hash8>.png`, deduplicated (decision A) |
| 11 | Exports | Done ([ADR-0031](adr/0031-markdown-export.md)): self-contained HTML (images, KaTeX fonts and diagrams inlined, a report of what wasn't carried over) and PDF printed by WebView2. Spike: 1 MiB slice PDF 47.5 s, 1,131 pages; 4 and 20 MiB didn't finish (renderer 5.9 / 12.6 GB). User decision 2026-10-16: PDF refused over a 10 MiB page, `krilla` in M6. Export long tasks up to 3.2 s (RISKS #17) |
| 12 | Corpora, fixtures and fuzzing | Done: fuzz targets `md_scan`, `md_incremental`, `md_sanitize`, `highlight` (every grammar), seeds, nightly matrix. Found and fixed: a BOM mid-document parsed differently in a stretch (ADR-0027 addendum); Markdown links skipped the href allowlist (ADR-0028 addendum); scheme check cut at 16 characters; front matter moved by an edit; footnote labels that depended on the stretch; search missed matches longer than the chunk overlap (M1 code); a SEGV in the Markdown grammar's C scanner (and the same calls in Bash) and a leak in SQL's, patched and vendored ([ADR-0034](adr/0034-patched-grammars.md)). Fuzz workflow green at `236c2b8`, all eight targets |
| 13 | Benchmarks | Done: `md-20m`, `md-20m-edit`, `tools/bench/refsim-md.ps1`, `md` rows in `bench/budgets.json`; criterion `crates/markdown/benches/markdown.rs` (4 MiB index 155 ms, keystroke + undo 1.16 ms). Final ref-sim 2026-10-17: MD 11/11, TXT 21/21, M0 11/11 |
| 14 | WebDriver UI flows | Done: 15 Markdown flows (adds export), 27 in all, pass locally. The M1 decode-progress flow no longer races startup; the table-helper flow waits for the view before saving |
| 15 | Carried from M1 | Done: webview tile-bitmap cache reported to the governor (`frontend_cache.rs`; bench: 160 MiB counted on both sides after scrolling); command-line open failure shown on the welcome screen; reopen with another encoding shows progress with Cancel (UI flows 10 and 11) |
| 16 | M2 report | Done: [M2_REPORT.md](M2_REPORT.md) |

Decided 2026-10-15 (ADR-0024 addendum): the Markdown keystroke → preview row is judged on keystrokes the platform didn't stall (86 % in ref-sim: p95 22–24 ms, max 29–31 ms, 33.3 ms effective); the 14 % that follow a pause and wait 150–370 ms for a frame (RISKS #12) are reported with every run.

## M1 tasks

| # | Task | Status |
|---|---|---|
| 1 | Sources | Done: mapped + guarded copies, chunked for remote files; [ADR-0007](adr/0007-source-access.md) |
| 2 | Encodings | Done: detection, segmented transcoding with raw copy on save, decode progress and cancel; [ADR-0009](adr/0009-encodings-and-transcoding.md) (350–560 MB/s cold) |
| 3 | Line index | Done; nth-break lookups are a binary search over prefix sums |
| 4 | Piece tree | Done: persistent treap, proptests, criterion benches; [ADR-0008](adr/0008-text-buffer-piece-tree.md) |
| 5 | Editing model | Done: multi-cursor, column selection, wrap layout, undo/redo; proptest (undo exact, journal replay) |
| 6 | Search and replace | Done: chunked parallel search (proptest vs. whole buffer), replace-all as one undo step |
| 7 | Save pipeline | Done: two-phase atomic save with the §5.8 sequence; byte-exact round trips across 11 cases; writes overlap reads (0.90–1.05× a raw write in the core) |
| 8 | Journal | Done: kill test lost 0 of 953 keystrokes |
| 9 | External changes | Done: stamp + fingerprint classification, reload/keep/compare, edit lock |
| 10 | TXT surface spike | Done: DOM rows with local echo; [ADR-0023](adr/0023-txt-surface-dom-rows.md) |
| 11 | Host integration | Done (`src-tauri/src/txt.rs`) |
| 12 | Editor frontend | Done (`src/txt/`): local echo, offset-independent row keys, latest-wins fetch queue |
| 13 | Fuzzing | Done: 4 cargo-fuzz targets, 10 min each (1 bug found and fixed), nightly workflow |
| 14 | Benchmarks | Done: two ref-sim runs (`bench/results/m1/`), criterion, transcoding, Edge idle probe |
| 15 | WebDriver UI flows | Done: 9/9 pass locally and in CI (Windows runner). The runner's WebView2 ignored msedgedriver's switches; the app now applies them under tauri-driver (ADR-0020 addendum) |
| 16 | M1 report | Written ([M1_REPORT.md](M1_REPORT.md)); awaiting the decision on platform frame stalls |

Found and fixed during M2: TXT `fetchAt` jumped back two screens on every refetch (TXT re-measured in ref-sim: 21/21); find-previous could return a match the forward search skips when matches overlap (nightly fuzz, regression input kept); after a save, the Markdown state read the text through a stale snapshot (a readers gate in `txt.rs` now covers the save's hand-over).

Open decision (2026-10-09, decided 2026-10-12 as ADR-0024): platform frame stalls (RISKS #12). Typing latency, the earlier open issue, is fixed: ref-sim p95 8.1–9.4 ms with local echo (ADR-0023).

## M0 tasks

| # | Task | Status |
|---|---|---|
| 1 | Workspace scaffold | Done (85ac033) |
| 2 | License and dependency gates | Done (85ac033) |
| 3 | CI | Done: [Offset54/quire](https://github.com/Offset54/quire) (private; public since 2026-10-13, ADR-0025); Windows runs every gate, macOS and Linux build and test; first run green on all three |
| 8 | Tracing | Done: Chrome-trace recorder in `quire-core::trace`, frontend measures and long tasks merged (`QUIRE_TRACE=<file>`) |
| 5 | Job scheduler | Done: `quire-core::scheduler`, 12 unit tests + a model-based property test, criterion benches |
| 6 | Memory governor | Done: `quire-core::memory` (per-document and global budgets, background-first and cheapest-class-first eviction, `LruCache`), 9 tests incl. a budget-invariant proptest and a concurrency accounting check |
| 4 | Shell hardening | Done: command allowlist + per-window capability, navigation/new-window/download blocking (unit-tested), Windows manifest (longPathAware, PerMonitorV2, UTF-8 code page), window-state restore, ICU MessageFormat catalog (`t()`) |
| 7 | Tile transport spike | Done: [ADR-0004](adr/0004-tile-transport.md). Windows uses WebView2 SharedBuffer (raw 512 px tile 2.7 ms p50, full 1080p repaint 26 ms); fallback is `invoke` + QOI |
| 9 | Compositor and virtual scroller | Done: [ADR-0005](adr/0005-virtual-scroll-model.md). `DocumentSurface` (tile + overlay canvases, custom scrollbar, LRU bitmaps), view service, synthetic 10M-row list behind `bench-fixtures`. Raw dev machine: steady scroll and fling 0 dropped frames at 144 Hz, 0 placeholders |
| 10 | Performance HUD | Done: Ctrl+Shift+H overlay (frame graph with 16.7/50 ms lines, long tasks, queue depths, job counters, memory caches, per-surface tile counters, last 10 jobs); verified live by the `hud` bench scenario |
| 11 | Fixture generators | Done: `tools/fixtures`, all seven §4 fixtures, deterministic (verified), validated by qpdf, Word (1,000 pages) and PowerPoint (500 slides) |
| 12 | Benchmark harness | Done: [ADR-0020](adr/0020-benchmark-and-test-harness.md), `tools/refsim` + `tools/bench/refsim.ps1` (ADR-0021 profile), `bench/budgets.json`, `compare.mjs`, CI A/B micro-benchmarks |
| 13 | Cold-start work | Investigated: startup spans; bare-WebView2 floor spike (449 ms pinned); resident-process variant 137 ms. Budget revised by decision ([ADR-0022](adr/0022-startup-budget-and-webview2-floor.md)): warm ≤ 0.75 s, cold ≤ 1.7 s; re-run passes (warm 457 ms) |
| 14 | M0 report | Done: [M0_REPORT.md](M0_REPORT.md); ADRs 0001–0006 and 0018–0022 written |

## Decisions recorded on plan approval (2026-10-08)

- License allowlist extended with Unicode-3.0, BSL-1.0, CC0-1.0, 0BSD; OFL-1.1 for bundled fonts and FTL for PDFium's FreeType. MIT-0 and `Apache-2.0 WITH LLVM-exception` are also allowed, since both are strictly more permissive than an already-allowed license.
- Hosting: private GitHub repo with Actions. Made public on 2026-10-13 ([ADR-0025](adr/0025-public-repository.md)).
- Benchmarking: ref-sim profile on this machine (ADR-0021) instead of a reference laptop.
- License allowlist: Unicode-DFS-2016 (the predecessor of Unicode-3.0, same grant) approved on 2026-10-13 for comrak's `finl_unicode` (ADR-0026).
- TypeScript is pinned to 6.0.x: typescript-eslint 8.71 supports `<6.1`, so TS 7 waits for it.

## Benchmarks vs. budget

### Micro-benchmarks (criterion, raw dev machine, AC power, 2026-10-08)

These are component costs, not §4 scenarios, so they aren't derated.

| Benchmark | Median | Notes |
|---|---|---|
| Scheduler: submit + wait, Interaction, idle pool | 5.7 µs | Hand-off cost on the input path; 0.04 % of the 16 ms budget |
| Scheduler: 10k no-op jobs, mixed priorities | 16.7 ms (≈ 598k jobs/s) | 4 workers, 1 reserved |
| Scheduler: coalescing hit | 130 ns | Duplicate tile request while queued |

### Tile transport (spike, raw dev machine, AC, 2026-10-08; details in ADR-0004)

| Path | Single 512 px tile p50 / p95 | Full 1920×1080 repaint p50 |
|---|---|---|
| WebView2 SharedBuffer, raw | 2.7 / 4.2 ms | 26 ms |
| `invoke`, raw | 12.5 / 13.4 ms | 83 ms |
| `invoke`, QOI (text tiles) | 3.6 / 5.2 ms | 34 ms |

### Synthetic 10M-row list (raw dev machine, AC, 144 Hz, 2026-10-08; ADR-0005)

| Phase | Frame interval p95 / max | Dropped frames | Frames with placeholders |
|---|---|---|---|
| Steady scroll 2,400 CSS px/s | 7.1 / 7.3 ms | 0 | 0 % |
| Fling 24,000 CSS px/s | 7.1 / 7.3 ms | 0 | 0 % |
| Thumb drag across all rows | 7.0 / 7.4 ms | 0 | 100 % (allowed) |

### Ref-sim, first run (2026-10-08, commit before task 13)

Profile ref-sim-v1: P-cores 0–3 pinned, 8 logical CPUs reported, 1920×1080 device px, D_cpu 1.5, D_io 1.74 (cold read 3480 MB/s). Effective budget = budget ÷ factor.

| Budget | Value | Kind | Effective budget | Result |
|---|---|---|---|---|
| startup.warm | 480.4 ms | cpu | 266.67 | **FAIL** |
| startup.cold | 936.1 ms | mixed | 574.79 | **FAIL** |
| scroll.steady.p99 | 8.6 ms | raw | 16.7 | pass |
| scroll.steady.max | 8.9 ms | raw | 50.0 | pass |
| scroll.fling.p99 | 8.7 ms | raw | 16.7 | pass |
| scroll.fling.max | 8.9 ms | raw | 50.0 | pass |
| scroll.drag.p99 | 8.5 ms | raw | 16.7 | pass |
| scroll.drag.max | 8.8 ms | raw | 50.0 | pass |
| scroll.composeWork.p95 | 0.9 ms | cpu | 5.33 | pass |
| scroll.composeWork.max | 2.7 ms | cpu | 5.33 | pass |
| scroll.longTasks | 0.0 tasks | zero | 0 | pass |

### §4 scenarios

No official results yet: the harness and ref-sim profile come in task 12.

Early signal (one unofficial run, raw dev machine, not ref-sim, 2026-10-08): the empty release app's first frame landed **~525 ms** after core start on a warm launch. The warm budget is ≤ 400 ms (and ≤ ~267 ms after the ref-sim derating), so task 13 has real work to do.

## Dev environment (as of 2026-10-08)

| Tool | Version | Used for |
|---|---|---|
| Rust stable | 1.99.0, pinned in `rust-toolchain.toml` | Everything |
| Rust nightly | 1.101.0-nightly (2026-10-07), minimal profile | `cargo +nightly fuzz` only |
| cargo-deny | 0.20.2 | License and advisory gate |
| tauri-cli | 2.12.1 | Shell |
| cargo-fuzz | 0.13.2 | Fuzzing |
| MSVC | Visual Studio Build Tools 2026 18.5, Windows SDK 10.0.26100 | Linking |
| WebView2 | 154.0.4258.62 | UI runtime |
| Node / npm / pnpm | 24.15.0 / 11.18.0 / 12.10.1 | Frontend |
| Git | 2.54.0 | |
| Python | 3.14.4 | Available for tooling scripts |
| .NET | Runtimes 8.0 and 10.0, no SDK | The OpenXmlValidator CLI needs the SDK (M4) |

Needed later and not installed yet: PDFium prebuilt binaries and `qpdf` (M3), the .NET SDK (M4), and LibreOffice (M6; optional external converter, never bundled).

## Dev machine vs. reference machine

| | Dev machine | Reference machine (§0) |
|---|---|---|
| CPU | Intel Core Ultra 7 258V (2024): 4 P-cores + 4 LP E-cores, 8 threads, no SMT | 4 cores / 8 threads, 2021-era laptop |
| RAM | 32 GB | 16 GB |
| GPU | Intel Arc 140V (integrated) | Integrated |
| Storage | NVMe SSD | NVMe SSD |
| Display | 2560×1600 at 150 % | 1920×1080 at 150 % |

Benchmarks that count against §4 run in the ref-sim profile ([ADR-0021](adr/0021-reference-machine-emulation.md)) and are labeled "ref-sim", never "reference".

## Known gaps

- The Microsoft IMEs (Korean, Japanese, Chinese) haven't been checked by hand; the UI flow replays the Korean IME's composition events.

- Windows long paths are enabled on this PC, and git uses `core.longpaths = true` for this repo. The app must still declare `longPathAware` in its manifest (§9), because users' machines may have the setting off.
