# Progress

| | |
|---|---|
| Phase | Pre-M0. The §13 plan is delivered (ARCHITECTURE.md, adr/README.md, RISKS.md, M0_PLAN.md) and waiting for approval. No application code. |
| Product name | Codename `quire` (bundle id `dev.quire.app`) |
| Last updated | 2026-10-08 |

## Milestones

| Milestone | Status | DoD |
|---|---|---|
| M0 Foundations | Not started; waiting on plan approval | n/a |
| M1 TXT engine | Not started | n/a |
| M2 Markdown | Not started | n/a |
| M3 PDF | Not started | n/a |
| M4 DOCX | Not started | n/a |
| M5 PPTX | Not started | n/a |
| M6 PPT, export and print | Not started | n/a |
| M7 Hardening and release | Not started | n/a |

## Benchmarks vs. budget

None yet. The benchmark harness is part of M0.

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

Benchmarks run on this machine are not reference-machine numbers. They will be labeled with this spec until a reference-class machine or a calibrated throttle is decided on (to be raised in the §13 plan).

## Known gaps

- Windows long paths are enabled on this PC, and git uses `core.longpaths = true` for this repo. The app must still declare `longPathAware` in its manifest (§9), because users' machines may have the setting off.
