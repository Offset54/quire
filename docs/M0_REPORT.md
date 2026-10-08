# M0 Foundations: report

- Date: 2026-10-08
- Commits: `e7fb265` … HEAD on `main` ([Offset54/quire](https://github.com/Offset54/quire))
- CI: green on Windows, macOS and Linux

## Definition of Done (§11)

| DoD item | Status | Evidence |
|---|---|---|
| The empty app starts within budget | **Met under the revised budget** (warm ≤ 0.75 s, cold ≤ 1.7 s; decided 2026-10-08, [ADR-0022](adr/0022-startup-budget-and-webview2-floor.md)). Re-run: warm 457 ms p50 against an effective 500 ms. Before the revision: | Ref-sim warm 480 ms p50 against an effective 267 ms (budget 400 ms ÷ 1.5); cold 936 ms against 575 ms. A bare WebView2 host with no Tauri needs 449 ms on this machine, so the budget is below the floor of the stack §5.1 mandates. Tauri and our code add only 30–40 ms. With a resident WebView2 browser process the first frame takes 137 ms. |
| A synthetic 10M-row list scrolls at 60 fps | **Met** | Ref-sim (4 P-cores, 1920×1080): steady scroll p99 frame 8.6 ms, fling 8.7 ms, thumb drag over the whole document 8.5 ms; max frame 8.9 ms; no long tasks; compose work 0.9 ms p95. Placeholders on 0 % of steady and fling frames (the thumb drag shows placeholders while moving, which §4 allows). `bench/results/m0/refsim-2026-10-08-*.json`, ADR-0005. |
| Core → screen tile round trip measured and documented in an ADR | **Met** | [ADR-0004](adr/0004-tile-transport.md): four transports × three encodings × two contents × two tile sizes, two runs. Decision: WebView2 SharedBuffer raw (2.7 ms per 512 px tile, 26 ms full repaint), with `invoke` + QOI in a worker as the fallback. |

## Scope checklist (§11 M0)

| Item | Where |
|---|---|
| Workspace, lint, format, license gates | Cargo + pnpm workspaces; clippy `-D warnings`, rustfmt, ESLint (type-checked), Prettier; cargo-deny + JS gate ([ADR-0019](adr/0019-license-gate.md)) |
| CI, Windows first, then macOS and Linux | `.github/workflows/ci.yml`: Windows runs every gate; macOS and Linux build and test; license and advisory job; A/B micro-benchmark job on PRs |
| Tauri shell | [ADR-0001](adr/0001-app-shell-tauri.md). Strict CSP, command allowlist + capabilities, navigation/new-window/download guards, Windows manifest (longPathAware, PerMonitorV2, UTF-8), window-state restore, ICU MessageFormat strings ([ADR-0018](adr/0018-i18n.md)) |
| Job scheduler with cancellation | `quire-core::scheduler` ([ADR-0003](adr/0003-process-and-scheduler.md)) |
| Memory governor | `quire-core::memory`: per-document and global budgets, background-first and cheapest-class-first eviction, governed `LruCache` |
| Binary IPC path | SharedBuffer / Channel tile sinks (`apps/desktop/src-tauri/src/transport.rs`), [ADR-0004](adr/0004-tile-transport.md) |
| Virtual scroller with custom scrollbar | `DocumentSurface` + `ScrollModel`, [ADR-0005](adr/0005-virtual-scroll-model.md) |
| `tracing` and the performance HUD | `quire-core::trace` (Chrome trace, frontend timings merged; `QUIRE_TRACE`), HUD on Ctrl+Shift+H |
| Fixture generators and benchmark harness | `tools/fixtures` (all seven §4 fixtures, deterministic, validated with qpdf/Word/PowerPoint); `tools/refsim` + `tools/bench/refsim.ps1` ([ADR-0020](adr/0020-benchmark-and-test-harness.md), [ADR-0021](adr/0021-reference-machine-emulation.md)) |

## Tests

- Rust: 33 tests across quire-core (scheduler incl. a model-based proptest, memory governor incl. a budget-invariant proptest and a concurrency check, tile keys, tracing), quire-raster and quire-desktop (navigation guard, transport encoding).
- Frontend: 13 vitest tests (scroll model at 3·10^8 px, tile planner, wire protocol, QOI decoding).
- Benchmarks: criterion (scheduler); in-app scenarios `startup`, `scroll-list` and `hud`.

## Deviations from the brief

1. **End-to-end benchmarks run in-app, not over WebDriver** ([ADR-0020](adr/0020-benchmark-and-test-harness.md)). It's more precise (rAF timestamps in the page, process creation from the OS) and doesn't depend on msedgedriver versions. WebDriver is planned for UI-flow tests from M1.
2. **Ref-sim instead of a reference laptop** ([ADR-0021](adr/0021-reference-machine-emulation.md)), as you approved: exact viewport, P-core pinning, derated thresholds, verified cold cache.
3. **Estimate.** M0 was planned at 39.5 engineer-days. The work is done except the startup decision.

## Problems found and fixed during M0

- Re-requests of tiles still in flight flooded the UI thread during scrolling (4,125 tiles in 6 s instead of ~180): fixed with an in-flight window in the core.
- The prefetch window was wider than the drop window, so prefetched tiles were thrown away and 98 % of fling frames showed placeholders: both windows now come from one function, with a test.
- Tauri Channel messages are ordered per invocation; reusing one Channel hangs. Fixed, and documented in ADR-0004.
- Word dropped 11 of 999 manual page breaks in the DOCX fixture; switched to page-break-before (exactly 1,000 pages, verified in Word).
- The Windows SxS manifest parser rejects comments and non-ASCII text; the non-Windows spike build needed a PNG icon.

## Known gaps carried forward

- Overlapping document opening with WebView2 startup (ADR-0022) starts in M1; the opt-in fast-start process comes in M7.
- macOS and Linux transport and scroll performance are unmeasured on real hardware (risk #5).
- The webview's tile bitmap cache is capped by count but not reported to the core memory governor (PROGRESS.md).
- **"Cold" starts are only partly cold on a machine in use.** Purging can't evict pages that other Edge/WebView2 processes have mapped. 11 such processes were running during the measured runs, and the cold samples were bimodal (464–1,010 ms).
- WebDriver-based UI-flow tests: from M1.

## Decision taken (2026-10-08)

ADR-0022 was accepted with Options 3 + 4 + 1:
- the startup budget is revised to warm ≤ 0.75 s and cold ≤ 1.7 s;
- document opening overlaps WebView2 startup from M1;
- an opt-in fast-start process comes in M7.

M0 is complete. All three DoD items are met under the revised budget.
