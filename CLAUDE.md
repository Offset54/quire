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

## State (2026-10-13)

- **M0 and M1 are complete** (`docs/M0_REPORT.md`, `docs/M1_REPORT.md`). Ref-sim run 6 met all 21 TXT budgets. **M2 (Markdown): plan approved 2026-10-13** (`docs/M2_PLAN.md`, including the user's decisions A–D); execution starts at task 1 (parser spike) with task 15 alongside.
- Platform frame stalls (RISKS #12): on this machine the display path stalls 90–500 ms after ~0.6 s with no frames, in plain Edge too (`tools/bench/edge-idle-probe.mjs`). User decision (ADR-0024): budgets are judged on our own frame work; the bench subtracts pure waits for a requested frame (`apps/desktop/src/bench/stalls.ts`) and reports raw values and stalls next to them. Never subtract time that could be the host (IPC). Results carry `platformStalls`.
- Save ratio (user decision, M1_PLAN): save vs. the mean of raw writes right before and after, median of 5 runs. Raw 2 GiB writes on this SSD vary 1.1–2.4 s, worse after many test writes; let the disk rest before ref-sim runs.
- Fuzzing: `cd fuzz; cargo +nightly fuzz run <target>`; on Windows put the MSVC ASan runtime dir (`VC\Tools\MSVC\<ver>\bin\Hostx64\x64`) on PATH.
- Startup budget revised by the user (ADR-0022): warm ≤ 0.75 s, cold ≤ 1.7 s. M1 opens a command-line file before the webview is ready (done; measure with the `launch-2g` scenario). M7: opt-in "fast start" resident process.
- Webview caches count against the memory governor through `memory_report` (`src-tauri/src/frontend_cache.rs`): the frontend reports bytes, the governor answers with a `memory-trim` event. Use it for any new webview-side cache (preview images in M2, PDF tiles in M3).
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
- `tools/webdriver/`: msedgedriver matching WebView2 + `run-ui-tests.ps1` (tauri-driver, `tests/ui/txt-flows.test.mjs`).
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
