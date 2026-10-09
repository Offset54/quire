# 0020. Benchmark and test harness

- Status: Accepted
- Date: 2026-10-08

## Context

§10 asks for criterion micro-benchmarks, plus end-to-end scenario benchmarks that "drive the real app through WebDriver" and record the §4 metrics. Results are committed as JSON, and CI fails on regressions over 10 %. ADR-0021 adds the ref-sim profile. M0 needs this for its own DoD: startup within budget, and the 10M-row list at 60 fps.

## Decision

1. **Micro-benchmarks:** criterion (MIT/Apache-2.0) per crate, under `benches/`.
2. **End-to-end scenarios run in-app, not over WebDriver.** The `bench-fixtures` build (cargo feature plus `vite --mode bench`, built into `target/bench`) contains scenario modules:
   - the frontend runs the scenario named by `QUIRE_BENCH`;
   - it measures from inside the page (rAF timestamps, Long Tasks API, `performance.timeOrigin`);
   - the core adds the OS process-creation time;
   - the results are written as JSON (`bench_report`) and the app exits.

   Scenarios so far: `startup`, `scroll-list`, `hud`.
3. **Ref-sim orchestration:** `tools/bench/refsim.ps1` with the `tools/refsim` helper.
   - Pins every process in a Job Object (WebView2 included) to the performance cores.
   - Reports 8 logical CPUs to the app.
   - Fixes the window at 1920×1080 device px.
   - Purges files for cold starts and measures `D_io`.
   - Records calibration kernels, and checks results against `bench/budgets.json` with the derating factors.
4. **Regressions:** `tools/bench/compare.mjs` compares two ref-sim results (budget metrics, with a 0.5-unit noise floor) or two criterion baselines. In CI, PRs run the micro-benchmarks A/B: base and head back to back on the same runner, failing above 10 %.
5. **Fixtures:** `tools/fixtures` (task 11) generates every scenario input deterministically.

## Why in-app instead of WebDriver

| | In-app runner | WebDriver (tauri-driver + msedgedriver) |
|---|---|---|
| Frame timing | rAF timestamps in the page, the same clock the compositor uses | Polled over HTTP; cannot see individual frames |
| Startup | OS process creation to first painted frame | Needs a session handshake before the first command, which adds its own latency |
| Platforms | Same on Windows, macOS, Linux | tauri-driver has no macOS support |
| Version coupling | None | msedgedriver must match the installed WebView2 exactly |
| Product impact | Scenario code is compiled out of normal builds (feature flag + Vite mode) | None |

WebDriver is still the right tool for **UI-flow tests** (click Open, type, check the result), once there is UI to drive. It will be added in M1 for the TXT editor's flows, and is tracked in PROGRESS.md. For measuring §4 budgets, the in-app runner is more precise and has fewer moving parts.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| WebDriver for benchmarks | tauri-driver MIT/Apache-2.0; msedgedriver proprietary but free | See the table above |
| Playwright with CDP attached to WebView2 | Apache-2.0 | Works on Windows only, and CDP attach changes WebView2 startup flags, which would distort startup timings |
| Criterion on CI against a stored baseline | — | Runners differ from run to run. The A/B in one job compares like with like. |

## Consequences

- **Ref-sim scenario runs are local**, on the dev machine with AC power. CI runs only the A/B micro-benchmarks. The committed ref-sim results (`bench/results/m0/`) are the record of the §4 checks.
- **"Cold" is best effort on a machine in use.** Purging can't evict pages that other Edge or WebView2 processes have mapped. Each cold result records how many such processes were running.

## Addendum (M1, 2026-10-09): UI flows and fuzzing

- **UI flows** run through tauri-driver 2.1.0 (MIT/Apache-2.0) and the msedgedriver that matches the installed WebView2 (`tools/webdriver/`), with a minimal W3C client in `tests/ui/webdriver.mjs`. A CI job on Windows runs them against the release build.
- **Fuzzing** uses cargo-fuzz 0.13.2 (MIT/Apache-2.0) on a pinned nightly, with `libfuzzer-sys` 0.4 (`(MIT OR Apache-2.0) AND NCSA`; NCSA is LLVM's BSD-style license for libFuzzer). The `fuzz/` crate is its own workspace, outside the cargo-deny gate, and is never part of the shipped app. `cfg(fuzzing)` shrinks the search chunk and transcoding segment sizes so small inputs cross many seams. Seeds (`fuzz/seeds/`) and fixed crash inputs (`fuzz/regressions/`) are committed byte-exact; a nightly workflow runs each target for 10 minutes.
- **Bench diagnostics.** In-app scenarios also record long animation frames and the worst frame of short scrolls after idle periods, so frames lost outside our code (RISKS #12) can be told apart from our own work.
