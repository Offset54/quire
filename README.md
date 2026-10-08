# quire

Working codename for an offline-first desktop app that views and edits TXT, Markdown, PDF, DOCX, PPTX and PPT files, built with Tauri 2, a Rust core and a TypeScript frontend.

- Brief: [docs/MASTER_PROMPT.md](docs/MASTER_PROMPT.md)
- Status: [docs/PROGRESS.md](docs/PROGRESS.md) (pre-M0; no application code yet)
- Decisions: [docs/adr/](docs/adr/)

## Prerequisites (Windows)

- Visual Studio Build Tools with the C++ workload and a Windows 10/11 SDK
- WebView2 runtime (included with Windows 11)
- Rust via [rustup](https://rustup.rs). `rust-toolchain.toml` pins the version, and rustup installs it on first use.
- Rust nightly, used only for fuzzing: `rustup toolchain install nightly --profile minimal`
- Cargo tools: `cargo install --locked cargo-deny cargo-fuzz tauri-cli --version "^2"`
- Node.js 24+ and pnpm

Build and run instructions will be added in M0.
