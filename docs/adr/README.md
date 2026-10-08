# Architecture Decision Records

One file per decision, named `NNNN-short-title.md` and numbered in order. Numbers are never reused. A superseded ADR stays in place, with its status changed to "Superseded by NNNN".

Every dependency gets a line in an ADR, including its license (see MASTER_PROMPT §0 and §2).

## Index

The plan was approved on 2026-10-08. Entries stay **Proposed** until their own file records the decision and evidence. Each gets its own file when it's decided, with evidence. "Spike" means the decision waits on a time-boxed measurement. Licenses were checked from memory and must be re-verified, along with maintenance status, when a dependency is adopted.

| # | Decision | Proposed choice | Alternatives considered (license) | Decided in |
|---|---|---|---|---|
| 0001 | App shell | Tauri 2 (MIT/Apache-2.0) | Electron (MIT, Chromium BSD-3; ~150 MB and more RAM per window); Qt 6 (LGPL-3/commercial; excluded by §0); egui/iced (MIT/Apache; weak IME and a11y for rich UI); native per-OS UI (three codebases) | M0 |
| 0002 | Frontend chrome | React (MIT) for chrome only; imperative TS for surfaces | Solid (MIT); Svelte (MIT); vanilla TS | M0 |
| 0003 | Process and scheduling model | Core in-process; custom priority scheduler on std threads, rayon (MIT/Apache) for data-parallel work; PDFium and soffice in child processes | tokio everywhere (MIT; not built for CPU-bound priority work); in-process PDFium behind a mutex (no parallelism, no crash isolation) | M0 |
| 0004 | Tile transport (IPC) | [Accepted (Windows)](0004-tile-transport.md): WebView2 SharedBuffer, raw RGBA, 512 px tiles; fallback `invoke` + QOI decoded in a worker | JSON/base64 (banned by §5.6); localhost WebSocket (extra attack surface) | M0 (spike) |
| 0005 | Virtual scroll model | Custom scrollbar over a (unit, offset) model with 64-bit logical extents; DOM extent kept under browser caps | Native overflow scrolling (breaks above ~33M px) | M0 |
| 0006 | Rasterizer | tiny-skia (BSD-3) baseline; vello_cpu (Apache/MIT) behind a feature flag | skia-safe (MIT bindings, Skia BSD-3; heavy C++ build); Direct2D (Windows only); GPU vello (driver variance) | M0 (bench), revisit M4 |
| 0007 | Source access | memmap2 (MIT/Apache); pread + LRU chunk cache for network and cloud-placeholder files; in-page-error guard on Windows | Read whole file (violates §5.5); mmap everywhere (crashes on OneDrive/SMB I/O errors) | M1 |
| 0008 | Text buffer | Custom piece tree on a B-tree with cached byte/newline sums, over the mapped original plus an append-only add buffer | ropey (MIT; loads the whole file into RAM); crop (MIT; same); gap buffer (O(n) moves); red-black piece tree as in VS Code (no summary caching beyond one metric) | M1 |
| 0009 | Encoding detection | encoding_rs (MIT/Apache) + chardetng (MIT/Apache) | ICU charset detection via ICU4C (Unicode license; C++ build) | M1 |
| 0010 | Markdown parser | Decide between comrak (BSD-2) and pulldown-cmark (MIT) by spec pass rate, source positions and incremental reparse cost | markdown-rs (MIT; slower); markdown-it in JS (MIT; puts the document in the JS heap) | M2 |
| 0011 | PDF engine | PDFium (BSD-3 + permissive third parties incl. FreeType FTL) via pdfium-render (MIT/Apache) with prebuilt binaries | MuPDF (AGPL; excluded); Poppler (GPL; excluded); pdf.js (Apache-2.0; JS heap, slow at 1 GB); hayro (Apache/MIT; young, no forms or annotations) | M3 |
| 0012 | PDF structure ops and export | lopdf (MIT) for page ops; qpdf (Apache-2.0) for checks and linearization; krilla (MIT/Apache) for export | PDFium import APIs alone (enough for reorder, not for full merge); pdf-writer (MIT/Apache; lower level) | M3 / M6 |
| 0013 | OOXML I/O | zip (MIT) with raw-entry copy; quick-xml (MIT) pull parser; byte-range splice writer | rc-zip (MIT/Apache); roxmltree (MIT/Apache; DOM, whole part in memory); xml-rs (MIT; slower) | M4 |
| 0014 | Rich-text layout engine | Own engine on HarfRust (MIT) or rustybuzz (MIT), skrifa (MIT/Apache), fontique (Apache/MIT) and ICU4X (Unicode-3.0); evaluate parley (Apache/MIT) as a base | LibreOffice layout out of process (MPL-2.0; no incremental editing, no per-glyph control); cosmic-text (MIT/Apache; no pagination or tables); browser DOM layout (measure/paint mismatch, ruled out by §5.4) | M4 (spike) |
| 0015 | Bundled fonts | Carlito, Caladea, Liberation, Noto CJK (all OFL-1.1) | No bundled fonts (bad fidelity on systems without Office fonts) | M4 (needs OFL-1.1 approval) |
| 0016 | EMF/WMF | Own record-to-display-list converter | libuemf (GPL; excluded); delegating to the OS (Windows only) | M5 |
| 0017 | PPT import | External `soffice --headless` (MPL-2.0, user-installed, out of process) + own CFB quick look via `cfb` (MIT) | Apache POI HSLF (Apache-2.0; needs a JVM); native Rust parser (v2); commercial SDKs (not permissive) | M6 |
| 0018 | i18n | ICU MessageFormat via `@formatjs/intl` (MIT) in the UI; ICU4X (Unicode-3.0) in the core | i18next (MIT; not ICU MessageFormat by default); Fluent (Apache/MIT; different syntax) | M0 |
| 0019 | License gate | cargo-deny (MIT/Apache) with an explicit allowlist; `license-checker-rseidelsohn` (BSD-3) for JS | Manual review (doesn't scale) | M0 |
| 0020 | Test and bench harness | criterion (MIT/Apache), proptest (MIT/Apache), cargo-fuzz (MIT/Apache), WebDriver via tauri-driver (MIT/Apache) + msedgedriver | Playwright (Apache-2.0; can't drive WebView2 inside Tauri as directly) | M0 |
| 0021 | Benchmarking without a reference machine | [Accepted](0021-reference-machine-emulation.md): ref-sim profile (exact viewport, core pinning, derated thresholds, cold-cache purge, RAM ballast) | Raw dev numbers; power-plan frequency cap (system-wide setting); Job Object CPU-rate cap (adds stalls); VM; buying a reference laptop | M0 |

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context

## Decision

## Alternatives considered

| Option | License | Why not |
|---|---|---|

## Consequences

## Evidence

Benchmarks (p50/p95, with machine spec) or spike results that support the decision.
```
