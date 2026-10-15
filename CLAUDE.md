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

## State (2026-10-15, evening handoff)

- **M0 and M1 are complete** (`docs/M0_REPORT.md`, `docs/M1_REPORT.md`). **M2 (Markdown) in progress** (`docs/M2_PLAN.md`, user decisions A–D; task table in `docs/PROGRESS.md`, current as of 2026-10-15).
- **M2 done and pushed (main at the "Ref-sim and docs" commit after 30928b7):** tasks 1–10 and 15; 13 partly (criterion benches to do); 14 partly (14 Markdown UI flows, 26 in all, pass locally; export flow to do). ADRs 0024 addendum 2 (typing judged on own values, applied), 0029 (highlighting), 0030, 0032 (user decision: Unlicense allowed, Mermaid pinned 11.17.2 to keep EPL-2.0 `elkjs` out), 0033 (KaTeX in page, Mermaid in a sandboxed frame served by the host's `quire-frame` scheme; Tauri's `freezePrototype` is off because it runs in subframes on Windows; `src/freeze.ts` freezes the app's own prototype). Engine: new `Action::Replace` (one undo step). Fixed: an edit taken by `md_blocks`/`md_view` now pushes its provisional window (`apply_edit`); `refsim run --timeout` (a hung scenario fails the suite).
- **Ref-sim 2026-10-15 17:06 (30928b7, `bench/results/m2/README.md`):** MD 11/11, TXT 21/21. **M0 startup.warm fails: 542 ms vs 500 ms effective** (cold 491 passes; on 2026-10-08 warm was 457–480). Binary grew 18.6 → 51.4 MB with the grammars (RISKS #15). **Attribution in progress:** a bench build of 94a4cfc (before highlighting) was started in the extra worktree `C:\Users\MSJin\dev\quire-prehl` (git worktree; its `target/bench/release/quire.exe`). Next: run `pwsh tools/bench/refsim.ps1 -NoBuild -WarmRuns 7 -ColdRuns 5 -Out <scratch>` in that worktree, then again here, on an idle machine on AC power; compare warm p50. If the grammars cause it, this is an unmet budget: bring data and options to the user (fewer grammars, grammars in separate files loaded on first use, accept), never relax it quietly. Remove the extra worktree afterwards (`git worktree remove`).
- **Task 11 (exports) is work in progress on branch `m2-export-wip`** (pushed, not on main): host `src-tauri/src/export.rs` (images inlined as data URIs with a report; `write_file`; `print_pdf` via WebView2 PrintToPdf in a hidden window), `md_export_html`/`md_export_save` in `md.rs`, `ImageService::export_bytes`, frontend `src/md/export.ts` + `export.css` (KaTeX CSS with woff2 fonts inlined via `import.meta.glob ?inline`), `rich.ts` refactored (`renderMathElement`, `diagramFigure`, exported `DiagramFrame`, `loadKatex`), Export menu in the status bar, `MdController.exportTo(kind, path)` for UI flows. Compiles, clippy/typecheck/lint clean, unit test passes; **not yet run in the app**. To do: merge onto main, a UI flow (export HTML: check `<style>`, inlined image, KaTeX font data URL, diagram `<img>`, no `<script>`, report lists the tracker image; export PDF: file starts with `%PDF`), the PDF spike numbers (20 MiB fixture: time, pages) and **ADR-0031** (PDF engine: WebView2 print vs. krilla).
- **CI:** the M1 UI flow "large non-UTF-8 file shows decode progress" failed twice on the runner since the binary grew (decoding may finish before the banner shows); reruns pass. Make it robust once startup is understood.
- **Next, in order:** startup attribution (above); finish task 11; 12 (fuzz targets: scanner, incremental-vs-full, sanitizer, every tree-sitter grammar, RISKS #16); 13 remainder (criterion: block-index build, incremental re-parse); 14 remainder (export flow); update ADR-0030 numbers; 16 `docs/M2_REPORT.md` (DoD evidence, re-estimate). Fuzz workflow last green 2026-10-14 17:22 (run 38606170933).
  - Watch: RISKS #14 (huge list/HTML block is one DOM entry), #15 (binary size), #16 (C parsers); edits during a save are refused "busy" (watch list).
  - UI flows: `pwsh tools/webdriver/run-ui-tests.ps1 [-NoBuild] [-Tests md-flows]`. Under WebDriver `window.__quire.session` exposes `view` (send, primary, reveal) and `md` (preview, outline, tableAction, exportTo). Byte offsets in UI tests: index a Buffer, not a JS string.
  - Examples: `crates/markdown/examples/previewbench.rs`, `crates/highlight/examples/hlbench.rs`.
  - Release build ~3 min incremental; bench flavor via `tools/bench/run-app-bench.ps1 -Scenario none -BuildOnly` (target/bench). Ref-sim suites: `refsim-md.ps1 -NoBuild`, `refsim-txt.ps1 -NoBuild -Out …`, `refsim.ps1 -NoBuild -Out …` (M0 startup and scrolling). **Never compile, fuzz or test while a ref-sim runs.** Ref-sim refuses on battery.
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
