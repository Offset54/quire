# quire: working notes

- The spec is `docs/MASTER_PROMPT.md`, the verbatim brief. `[PRODUCT_NAME]` is the working codename `quire` (bundle id `dev.quire.app`) until a real name is chosen before M7.
- Re-read the brief sections for the current milestone before starting on it. Its §2 rules govern all work here: measure, don't assert; no fake completeness; preserve what you don't understand; one ADR per decision; keep `docs/PROGRESS.md`, `docs/RISKS.md` and `docs/adr/` current.
- Milestones go in §11 order. Each DoD is reported (a `docs/M<n>_REPORT.md`) before the next milestone starts.
- Windows first. The Rust toolchain is pinned in `rust-toolchain.toml`; JS uses pnpm (exact versions, `.npmrc save-exact`).
- Line endings are LF everywhere (`.gitattributes`). Files under `testdata/`, `tests/data/` and `fuzz/regressions/` are byte-exact test inputs: never normalize, reformat or re-encode them.
- Generated fixtures and fetched corpora go in `/.cache/`, which is ignored. Never commit them.

## How the user wants work done

- Work autonomously through a whole milestone. Stop only when a decision is genuinely theirs: scope, licensing, UX trade-offs, or a budget that can't be met (§2: bring data and options, never relax a budget quietly).
- Make small, focused commits and push to `origin main` (private repo Offset54/quire) as you go. Keep CI green.
- Keep the user posted with short progress lines during long work.

## State (2026-10-08)

- **M0 is complete** (`docs/M0_REPORT.md`). M1 (TXT engine) is next.
- Startup budget revised by the user (ADR-0022): warm ≤ 0.75 s, cold ≤ 1.7 s on the reference machine. Carried into later milestones:
  - **M1 onwards:** when launched with a file, open and render it in parallel with WebView2 startup.
  - **M7:** opt-in "fast start" resident process.
- **Also carried into M1:**
  - WebDriver UI-flow tests (ADR-0020).
  - Report the webview tile-bitmap cache to the memory governor before M3 (PROGRESS.md known gaps).

## Layout and key pieces

- `crates/core`: scheduler (priorities, generations, coalescing), memory governor + `LruCache`, tile types/`TileSource`, Chrome-trace recorder.
- `crates/raster`: tiny-skia `TileCanvas`.
- `apps/desktop`: Tauri host (`src-tauri`) and frontend.
  - Host: view service; SharedBuffer/Channel tile transport (ADR-0004); `bench-fixtures` feature with the synthetic list and in-app benchmarks.
  - Frontend: `DocumentSurface` compositor and virtual scroller (ADR-0005), HUD (Ctrl+Shift+H), i18n `t()`.
- `tools/fixtures`: deterministic §4 fixtures → `.cache/fixtures`; `validate.ps1` checks them with qpdf, Word and PowerPoint.
- `tools/refsim` + `tools/bench/refsim.ps1`: ref-sim profile (ADR-0021) and budget checks against `bench/budgets.json`.
- `tools/bench/run-app-bench.ps1`: build the bench flavor (into `target/bench`) and run one scenario.
- `tools/bench/compare.mjs`: regression comparison.
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
