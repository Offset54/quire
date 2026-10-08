# M0 Foundations: task breakdown

Estimates are in focused engineer-days for one engineer, and include tests and docs for each task. They'll be re-estimated from actuals at the M0 report.

DoD (§11): the empty app starts within budget (cold ≤ 1.0 s, warm ≤ 0.4 s); a synthetic 10M-row list scrolls at 60 fps; the core-to-screen tile round trip is measured and documented in an ADR.

| # | Task | Output | Est. (d) | Depends on |
|---|---|---|---|---|
| 1 | Workspace scaffold | Cargo workspace with `crates/core`, `crates/raster`, `apps/desktop` (Tauri 2 + React + strict TS, pnpm workspace); rustfmt, clippy `-D warnings`, ESLint, Prettier | 1.5 | none |
| 2 | License and dependency gates | `deny.toml` with the approved allowlist; JS license check; versions pinned (lockfiles committed) | 1 | 1, license answer |
| 3 | CI | GitHub Actions on Windows: build, test, lint, format, license gates. Then a macOS and Linux matrix (build + unit tests + scroll bench smoke) | 2 | 1, 2, hosting answer |
| 4 | Shell hardening | Strict CSP, minimal Tauri capabilities, no remote content, per-monitor DPI, `longPathAware` manifest, window-state restore, i18n string scaffold (ICU MessageFormat) | 2 | 1 |
| 5 | Job scheduler | Five priority queues, cancel tokens, generation numbers, stale-job dropping, coalescing, worker threads; property tests; criterion benches for enqueue/cancel overhead | 4 | 1 |
| 6 | Memory governor | Global byte budget, cache registration, priority eviction, background-tab shedding hook; tests under simulated pressure | 3 | 1 |
| 7 | Tile transport spike | Measure custom URI scheme, Channel, `invoke` Response and WebView2 SharedBuffer × raw BGRA / LZ4 / QOI, for 256 and 512 px tiles at the device pixel ratio; p50/p95 latency and MB/s; **ADR-0004** | 4 | 1, 8 |
| 8 | Tracing | `tracing` with Chrome-trace export; frontend marks merged into the same trace; long-task observer flags anything over 50 ms | 1.5 | 1 |
| 9 | Compositor and virtual scroller | Canvas tile layer + overlay layer; custom scrollbar over a (unit, offset) model with 64-bit extents; placeholders, never blank; the synthetic 10M-row list with core-rendered tiles; **ADR-0005** | 5 | 5, 7 |
| 10 | Performance HUD | Developer toggle showing frame times, queue depths, cache sizes and the last N jobs | 2 | 5, 6, 8, 9 |
| 11 | Fixture generators | `tools/fixtures`, deterministic seeds, output to `/.cache/fixtures`: 2 GB mixed TXT, 300 MB one-line JSON, 20 MB Markdown, 1,000-page DOCX, 500-slide PPTX with ~500 MB media, 5,000-page PDF (linearized and not, via qpdf), 1,000-page scanned PDF. Each fixture is validated (opens in its authoring app or passes `qpdf --check`) | 5 | 1 |
| 12 | Benchmark harness | criterion setup; end-to-end runner over WebDriver (tauri-driver + msedgedriver) recording §4 metrics; ref-sim profile per ADR-0021 (job object P-core pinning, derating, verified cold cache, RAM ballast, power check, `quire-calibrate`); results as JSON with profile and machine spec; compare against baseline, fail CI on > 10 % regression | 6 | 3, 8 |
| 13 | Cold-start work | Measure and meet cold ≤ 1.0 s / warm ≤ 0.4 s for the empty app; lazy-load non-critical UI | 1.5 | 4, 12 |
| 14 | M0 report | ADRs 0001–0006, 0018–0020 finalized; ARCHITECTURE.md updated with measured numbers; PROGRESS.md and RISKS.md updated; DoD evidence | 1 | all |
| | **Total** | | **39.5** | |

With a 20 % contingency, M0 takes about 47 working days (roughly 9.5 weeks) of one engineer's time.

## Order

1. Tasks 1, 2 and 8, then the 4 / 5 / 6 / 11 group, which are independent of each other.
2. Task 7 (the spike) before task 9, so the scroller is built on the transport that wins.
3. Tasks 12 and 13 last, followed by the report.

Fixture generation (task 11) can run in parallel with everything else. It's sized here for M0 because the §11 scope requires all generators up front, even though most fixtures are first used in later milestones.
