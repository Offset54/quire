# Progress

| | |
|---|---|
| Phase | M0 Foundations, in progress. Plan approved 2026-10-08. |
| Product name | Codename `quire` (bundle id `dev.quire.app`) |
| Last updated | 2026-10-08 |

## Milestones

| Milestone | Status | DoD |
|---|---|---|
| M0 Foundations | In progress: tasks 1, 2 and 8 done; see the task list below | Not yet |
| M1 TXT engine | Not started | n/a |
| M2 Markdown | Not started | n/a |
| M3 PDF | Not started | n/a |
| M4 DOCX | Not started | n/a |
| M5 PPTX | Not started | n/a |
| M6 PPT, export and print | Not started | n/a |
| M7 Hardening and release | Not started | n/a |

## M0 tasks

| # | Task | Status |
|---|---|---|
| 1 | Workspace scaffold | Done (85ac033) |
| 2 | License and dependency gates | Done (85ac033) |
| 3 | CI | Blocked: needs `gh auth login` (you) to create the private repo |
| 8 | Tracing | Done: Chrome-trace recorder in `quire-core::trace`, frontend measures and long tasks merged (`QUIRE_TRACE=<file>`) |
| 4–7, 9–14 | See [M0_PLAN.md](M0_PLAN.md) | Not started |

## Decisions recorded on plan approval (2026-10-08)

- License allowlist extended with Unicode-3.0, BSL-1.0, CC0-1.0, 0BSD; OFL-1.1 for bundled fonts and FTL for PDFium's FreeType. MIT-0 and `Apache-2.0 WITH LLVM-exception` are also allowed, since both are strictly more permissive than an already-allowed license.
- Hosting: private GitHub repo with Actions.
- Benchmarking: ref-sim profile on this machine (ADR-0021) instead of a reference laptop.
- TypeScript is pinned to 6.0.x: typescript-eslint 8.71 supports `<6.1`, so TS 7 waits for it.

## Benchmarks vs. budget

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

- Windows long paths are enabled on this PC, and git uses `core.longpaths = true` for this repo. The app must still declare `longPathAware` in its manifest (§9), because users' machines may have the setting off.
