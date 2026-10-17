# quire: working notes

- The spec is `docs/MASTER_PROMPT.md`, the verbatim brief. `[PRODUCT_NAME]` is the working codename `quire` (bundle id `dev.quire.app`) until a real name is chosen before M7.
- Re-read the brief sections for the current milestone before starting on it. Its §2 rules govern all work here: measure, don't assert; no fake completeness; preserve what you don't understand; one ADR per decision; keep `docs/PROGRESS.md`, `docs/RISKS.md` and `docs/adr/` current.
- Milestones go in §11 order. Each DoD is reported (a `docs/M<n>_REPORT.md`) before the next milestone starts.
- Windows first. The Rust toolchain is pinned in `rust-toolchain.toml`; JS uses pnpm (exact versions, `.npmrc save-exact`).
- Line endings are LF everywhere (`.gitattributes`). Files under `testdata/`, `tests/data/` and `fuzz/regressions/` are byte-exact test inputs: never normalize, reformat or re-encode them.
- Generated fixtures and fetched corpora go in `/.cache/`, which is ignored. Never commit them.

## How the user wants work done

- Work autonomously through a whole milestone. Stop only when a decision is genuinely theirs: scope, licensing, UX trade-offs, or a budget that can't be met (§2: bring data and options, never relax a budget quietly).
- Make small, focused commits and push to `origin main` (public repo Offset54/quire, ADR-0025) as you go. Keep CI green.
- Keep the user posted with short progress lines during long work.

## State (2026-10-17, M2 closed)

- **M0, M1 and M2 are complete** (`docs/M0_REPORT.md`, `docs/M1_REPORT.md`, `docs/M2_REPORT.md`). Next is **M3 PDF**: write its plan first (as `docs/M1_PLAN.md`/`docs/M2_PLAN.md` were), re-reading the brief's PDF sections, and bring the plan's open decisions to the user before starting.
- **Final M2 evidence:** spec suite 1,342/1,352 (99.26 %); ref-sim at `6f67702`: MD 11/11, TXT 21/21, M0 11/11. Warm start passed at 488 ms of 500 effective (420–428 earlier the same day): a thin margin that moves with the machine's state; watch it in every M0 run (RISKS #15). CI and the fuzz workflow (eight targets) were green at `236c2b8`.
- **User decisions in M2:** A–D (M2_PLAN); Unlicense allowed, Mermaid pinned 11.17.2 (ADR-0032); keystroke and typing rows judged on unstalled frames (ADR-0024 addenda); PDF export refused over a 10 MiB page, `krilla` for PDF export in M6 (ADR-0031, 2026-10-16).
- **Patched grammars (ADR-0034):** tree-sitter-md, -bash and -sequel are vendored under `third_party/` with scanner fixes, through `[patch.crates-io]` in both the main and the fuzz workspace. Upgrading one means re-vendoring and re-applying its "quire patch"; drop a copy when upstream releases the fix. Upstream reports for them await the user's OK.
- **Follow-ups owed:** exports block the window (long tasks up to 3.2 s for 1 MiB; RISKS #17): slice the math loop, add progress and Cancel. PDF export is Windows-only until M7.
  - Watch: RISKS #14 (huge list/HTML block is one DOM entry), #15 (binary size, warm start), #16 (C parsers); edits during a save are refused "busy" (UI flows wait for the view before saving).
  - UI flows: `pwsh tools/webdriver/run-ui-tests.ps1 [-NoBuild] [-Tests md-flows]`; `-Tests export-spike` times large exports (`QUIRE_SPIKE_FIXTURE`). Under WebDriver `window.__quire` exposes `open(path)` and `session` with `view` (send, primary, reveal) and `md` (preview, outline, tableAction, exportTo). Byte offsets in UI tests: index a Buffer, not a JS string.
  - Examples: `crates/markdown/examples/previewbench.rs`, `crates/highlight/examples/hlbench.rs`; criterion: `cargo bench -p quire-markdown --bench markdown`.
  - Release build ~3 min incremental; bench flavor via `tools/bench/run-app-bench.ps1 -Scenario none -BuildOnly` (target/bench). Ref-sim suites: `refsim-md.ps1 -NoBuild -Out …`, `refsim-txt.ps1 -NoBuild -Out …`, `refsim.ps1 -NoBuild -Out …` (M0 startup and scrolling). **Never compile, fuzz or test while a ref-sim runs**, and run them on an idle machine (a run while the user was active had 500 ms first paints). Ref-sim refuses on battery.
  - Startup attribution method (if warm start fails again): alternate two bench builds launch by launch in ref-sim (`refsim run --pcores --logical 8 -- <exe>`, `QUIRE_BENCH=startup`, warm ms = `firstFrameEpochMs - processCreatedEpochMs`), 15+ pairs, with the bare WebView2 floor (`spikes/webview2-floor`) as a third arm; single runs drift too much to attribute.
- Platform frame stalls (RISKS #12): on this machine the display path stalls 90–500 ms after ~0.6 s with no frames, in plain Edge too (`tools/bench/edge-idle-probe.mjs`). User decision (ADR-0024): budgets are judged on our own frame work; the bench subtracts pure waits for a requested frame (`apps/desktop/src/bench/stalls.ts`) and reports raw values and stalls next to them. Never subtract time that could be the host (IPC). Results carry `platformStalls`.
- Save ratio (user decision, M1_PLAN): save vs. the mean of raw writes right before and after, median of 5 runs. Raw 2 GiB writes on this SSD vary 1.1–2.4 s, worse after many test writes; let the disk rest before ref-sim runs.
- Fuzzing: `cd fuzz; cargo +nightly fuzz run <target>`; on Windows put the MSVC ASan runtime dir (`VC\Tools\MSVC\<ver>\bin\Hostx64\x64`) on PATH. On Windows a panic aborts before libFuzzer saves the input, so targets call `save_on_panic::guard` (input in `artifacts/<target>/crash-panic`). Windows has no leak checker: leaks show only on the Linux runner, whose reports are symbolized; crash inputs are uploaded as run artifacts (`gh run download`). A fixed crash input goes into `fuzz/regressions/<target>/`.
- Startup budget revised by the user (ADR-0022): warm ≤ 0.75 s, cold ≤ 1.7 s. M1 opens a command-line file before the webview is ready (done; measure with the `launch-2g` scenario). M7: opt-in "fast start" resident process.
- Webview caches count against the memory governor through `memory_report` (`src-tauri/src/frontend_cache.rs`): the frontend reports bytes, the governor answers with a `memory-trim` event. Use it for any new webview-side cache (PDF tiles in M3).
- The user declined screen control (computer use) for checking the app: verify UI through the WebDriver flows (`pwsh tools/webdriver/run-ui-tests.ps1`) and the in-app bench scenarios instead.

## Layout and key pieces

- `crates/core`: scheduler (priorities, generations, coalescing), memory governor + `LruCache`, tile types/`TileSource`, Chrome-trace recorder.
- `crates/raster`: tiny-skia `TileCanvas`.
- `crates/text` (M1): sources (mapped with guarded copies, chunked for remote), encodings + transcoding, line index, persistent piece tree, buffer, layout (wrap rows), document (editing, multi-cursor), history, search/replace, file (open, two-phase atomic save), journal, watch (external changes), view (binary row messages).
- `apps/desktop`: Tauri host (`src-tauri`) and frontend.
  - Host: view service; SharedBuffer/Channel tile transport (ADR-0004); `bench-fixtures` feature with the synthetic list and in-app benchmarks.
  - Host `src/txt.rs`: TXT sessions and `txt_*` commands.
  - Frontend: `DocumentSurface` compositor and virtual scroller (ADR-0005), HUD (Ctrl+Shift+H), i18n `t()`; `src/txt/` TXT editor (`TxtView` DOM rows, session, chrome).
- `tools/fixtures`: deterministic §4 fixtures → `.cache/fixtures`; `validate.ps1` checks them with qpdf, Word and PowerPoint.
- `tools/refsim` + `tools/bench/refsim.ps1`: ref-sim profile (ADR-0021) and budget checks against `bench/budgets.json`.
- `tools/bench/run-app-bench.ps1`: build the bench flavor (into `target/bench`) and run one scenario.
- `tools/bench/compare.mjs`: regression comparison.
- `tools/bench/refsim-txt.ps1`: M1 TXT scenarios in ref-sim against the `txt` budgets (`txt-2g`, `txt-2g-search`, `txt-save`, `txt-300m`, `launch-2g`).
- `tools/webdriver/`: msedgedriver matching WebView2 + `run-ui-tests.ps1` (tauri-driver, `tests/ui/*.test.mjs`).
- `crates/markdown` (M2): block index and incremental parsing, analysis and tables, rendering with the sanitizer, preview windows; `crates/highlight`: tree-sitter highlighting; host `src/md.rs`, `src/export.rs`; frontend `src/md/`.
- `third_party/`: vendored, patched grammars (ADR-0034).
- `spikes/`: measurement spikes (not shipped).

## Commands

- Checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm format:check`, `pnpm lint`, `pnpm typecheck`, `pnpm test`, `pnpm run check:licenses`, `cargo deny check`.
- App: `cd apps/desktop; cargo tauri build --no-bundle` → `target/release/quire.exe`.
- Benchmarks: `pwsh tools/bench/refsim.ps1` (needs AC power; refuses on battery). Ref-sim results are not reference-machine numbers: label them "ref-sim".

## Environment gotchas (this Windows machine)

- New shells may lack tool paths. Prefix them: `$env:USERPROFILE\.cargo\bin` (cargo, cargo-tauri, cargo-deny, cargo-fuzz), `C:\Program Files\GitHub CLI` (gh), `C:\Program Files\qpdf 12.4.2\bin` (qpdf).
- Repo scripts are PowerShell 7 (`#Requires -Version 7`): run them with `pwsh`, not `powershell.exe`.
- The Bash tool's shell has failed to parse long heredocs that contain apostrophes. Write files with the file tools instead.
- In Python one-liners, always open files with `encoding="utf-8"` (the default code page is CP949 and mangles `§`).
- The Windows SxS manifest (`apps/desktop/src-tauri/windows-app-manifest.xml`) must stay plain ASCII without comments.
- Tauri Channel messages are ordered per invocation: one Channel per logical stream (ADR-0004).
- Office automation (`tools/fixtures/validate.ps1`) skips if Word or PowerPoint is already running. Never touch the user's open Office session.
- Under WebDriver, msedgedriver passes app arguments as `--<lowercased arg>`; the app accepts that form when it names an existing file.
- The GitHub runner's WebView2 ignores `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, so under tauri-driver the app applies msedgedriver's switches itself (`webdriver_browser_args`, ADR-0020 addendum). If you change the window builder's browser args, keep that path. On a UI-flow failure, `run-ui-tests.ps1` prints a process probe (WebView2 switches, listening ports) and the driver logs, and CI uploads them as `ui-flow-logs`. msedgedriver doesn't forward the app's stdout or stderr.
- Windows refuses to truncate a file we have mapped (os error 1224): tests that rewrite a file must drop the document first.
- Python edits through bash heredocs mangle `\r`/`\n` escapes: write scripts with the file tools and run them.
