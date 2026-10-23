# Architecture Decision Records

One file per decision, named `NNNN-short-title.md` and numbered in order. Numbers are never reused. A superseded ADR stays in place, with its status changed to "Superseded by NNNN".

Every dependency gets a line in an ADR, including its license (see MASTER_PROMPT §0 and §2).

## Index

The plan was approved on 2026-10-08. Entries stay **Proposed** until their own file records the decision and evidence. Each gets its own file when it's decided, with evidence. "Spike" means the decision waits on a time-boxed measurement. Licenses were checked from memory and must be re-verified, along with maintenance status, when a dependency is adopted.

| # | Decision | Proposed choice | Alternatives considered (license) | Decided in |
|---|---|---|---|---|
| 0001 | App shell | [Accepted](0001-app-shell-tauri.md): Tauri 2 (MIT/Apache-2.0) | Electron (MIT, Chromium BSD-3; ~150 MB and more RAM per window); Qt 6 (LGPL-3/commercial; excluded by §0); egui/iced (MIT/Apache; weak IME and a11y for rich UI); native per-OS UI (three codebases) | M0 |
| 0002 | Frontend chrome | [Accepted](0002-frontend-react-chrome.md): React (MIT) for chrome only; imperative TS for surfaces | Solid (MIT); Svelte (MIT); vanilla TS | M0 |
| 0003 | Process and scheduling model | [Accepted](0003-process-and-scheduler.md): Core in-process; custom priority scheduler on std threads, rayon (MIT/Apache) for data-parallel work; PDFium and soffice in child processes | tokio everywhere (MIT; not built for CPU-bound priority work); in-process PDFium behind a mutex (no parallelism, no crash isolation) | M0 |
| 0004 | Tile transport (IPC) | [Accepted (Windows)](0004-tile-transport.md): WebView2 SharedBuffer, raw RGBA, 512 px tiles; fallback `invoke` + QOI decoded in a worker | JSON/base64 (banned by §5.6); localhost WebSocket (extra attack surface) | M0 (spike) |
| 0005 | Virtual scroll model | [Accepted](0005-virtual-scroll-model.md): f64 device-px offset, custom scrollbar, frontend asks for missing tiles by priority, core suppresses in-flight duplicates | Native overflow scrolling (breaks above ~33M px) | M0 |
| 0006 | Rasterizer | [Accepted](0006-rasterizer.md): tiny-skia (BSD-3) baseline; vello_cpu (Apache/MIT) behind a feature flag | skia-safe (MIT bindings, Skia BSD-3; heavy C++ build); Direct2D (Windows only); GPU vello (driver variance) | M0 (bench), revisit M4 |
| 0007 | Source access | [Accepted](0007-source-access.md): memmap2 (MIT/Apache) with guarded copies (SEH / SIGBUS → `io::Error`) and prefetch for bulk scans; pread + governed LRU chunks for network and cloud-placeholder files | Read whole file (violates §5.5); pread everywhere (slower cold scans, per-document cache); unguarded mmap (crashes on I/O errors) | M1 |
| 0008 | Text buffer | [Accepted](0008-text-buffer-piece-tree.md): persistent piece treap with cached byte/break sums (unknown until indexed), over the mapped original plus an append-only add buffer | ropey (MIT; loads the whole file into RAM); crop (MIT; same); gap buffer (O(n) moves); red-black piece tree as in VS Code (no summary caching beyond one metric) | M1 |
| 0009 | Encodings | [Accepted](0009-encodings-and-transcoding.md): encoding_rs (MIT/Apache) + chardetng (MIT/Apache); UTF-8 in place, others decoded once in parallel segments, raw copy of untouched ranges on save | ICU charset detection via ICU4C (Unicode license; C++ build) | M1 |
| 0010 | Markdown parser | Recorded as [0026](0026-markdown-parser.md) (the M2 plan numbered the M2 decisions 0026–0031) | markdown-rs (MIT; slower); markdown-it in JS (MIT; puts the document in the JS heap) | M2 |
| 0011 | PDF engine | Recorded as [0035](0035-pdf-engine.md) | MuPDF (AGPL; excluded); Poppler (GPL; excluded); pdf.js (Apache-2.0; JS heap, slow at 1 GB); hayro (Apache/MIT; young, no forms or annotations) | M3 |
| 0012 | PDF structure ops and export | To be recorded as 0040 (page operations, M3) and in M6 (export, `krilla` per ADR-0031): lopdf (MIT) or PDFium import for page ops; qpdf (Apache-2.0) for checks and linearization | PDFium import APIs alone (enough for reorder, not for full merge); pdf-writer (MIT/Apache; lower level) | M3 / M6 |
| 0013 | OOXML I/O | zip (MIT) with raw-entry copy; quick-xml (MIT) pull parser; byte-range splice writer | rc-zip (MIT/Apache); roxmltree (MIT/Apache; DOM, whole part in memory); xml-rs (MIT; slower) | M4 |
| 0014 | Rich-text layout engine | Own engine on HarfRust (MIT) or rustybuzz (MIT), skrifa (MIT/Apache), fontique (Apache/MIT) and ICU4X (Unicode-3.0); evaluate parley (Apache/MIT) as a base | LibreOffice layout out of process (MPL-2.0; no incremental editing, no per-glyph control); cosmic-text (MIT/Apache; no pagination or tables); browser DOM layout (measure/paint mismatch, ruled out by §5.4) | M4 (spike) |
| 0015 | Bundled fonts | Carlito, Caladea, Liberation, Noto CJK (all OFL-1.1) | No bundled fonts (bad fidelity on systems without Office fonts) | M4 (needs OFL-1.1 approval) |
| 0016 | EMF/WMF | Own record-to-display-list converter | libuemf (GPL; excluded); delegating to the OS (Windows only) | M5 |
| 0017 | PPT import | External `soffice --headless` (MPL-2.0, user-installed, out of process) + own CFB quick look via `cfb` (MIT) | Apache POI HSLF (Apache-2.0; needs a JVM); native Rust parser (v2); commercial SDKs (not permissive) | M6 |
| 0018 | i18n | [Accepted](0018-i18n.md): ICU MessageFormat via `@formatjs/intl` (MIT) in the UI; ICU4X (Unicode-3.0) in the core | i18next (MIT; not ICU MessageFormat by default); Fluent (Apache/MIT; different syntax) | M0 |
| 0019 | License gate | [Accepted](0019-license-gate.md): cargo-deny (MIT/Apache) with an explicit allowlist; `license-checker-rseidelsohn` (BSD-3) for JS | Manual review (doesn't scale) | M0 |
| 0020 | Test and bench harness | [Accepted](0020-benchmark-and-test-harness.md): criterion + in-app scenario runner + ref-sim orchestration; WebDriver reserved for UI-flow tests (M1) | Playwright (Apache-2.0; can't drive WebView2 inside Tauri as directly) | M0 |
| 0022 | Startup budget vs. the WebView2 floor | [Accepted](0022-startup-budget-and-webview2-floor.md): budget revised to warm ≤ 0.75 s / cold ≤ 1.7 s; overlap document opening with WebView2 startup (M1+); opt-in fast-start process (M7) | Native first frame; budget revision; overlap document work; change stack | M0 |
| 0021 | Benchmarking without a reference machine | [Accepted](0021-reference-machine-emulation.md): ref-sim profile (exact viewport, core pinning, derated thresholds, cold-cache purge, RAM ballast) | Raw dev numbers; power-plan frequency cap (system-wide setting); Job Object CPU-rate cap (adds stalls); VM; buying a reference laptop | M0 |
| 0023 | TXT surface | [Accepted](0023-txt-surface-dom-rows.md): virtualized DOM rows from core layout, local echo for typing | Core-rendered tiles; canvas fillText; contenteditable; EditContext | M1 |
| 0024 | Platform frame stalls in budgets | [Accepted](0024-platform-frame-stalls.md): judge frame and first-paint rows without frames the platform did not deliver; report the stalls separately. Addendum 2026-10-15: the Markdown keystroke row is judged on keystrokes not stalled | Count them against our budgets; ship a compositor keep-alive | M1 |
| 0025 | Repository visibility | [Accepted](0025-public-repository.md): public, so standard Actions runners are free | Stay private and trim CI; no Actions for a month; rewrite history first | M1 |
| 0026 | Markdown parser | [Accepted](0026-markdown-parser.md): comrak 0.56 (BSD-2-Clause), default features off, rendering through our own formatter overrides | pulldown-cmark (MIT; no extended autolinks, no renderer hooks); markdown-rs (MIT); our own parser | M2 (spike) |
| 0027 | Markdown incremental parsing | [Accepted](0027-markdown-incremental-parsing.md): top-level block index tiled by start lines, built in stretches, edits re-scanned until a block starts where an old one did; definitions found by probing; document-wide tables | Whole re-parse; our own block scanner; comrak end positions; a second parser for definitions | M2 |
| 0028 | Markdown sanitization and content loading | [Accepted](0028-markdown-sanitization-and-content-loading.md): allowlist sanitizer in the core for raw HTML; every image source through one policy; local images through the `quire-img` scheme at display size; remote content behind a per-document toggle, fetched by the host | ammonia; DOMPurify in the frontend; Tauri's asset protocol; the webview loading remote images; reqwest; ureq with rustls | M2 |
| 0029 | Code highlighting | [Accepted](0029-code-highlighting.md): tree-sitter in the core (`crates/highlight`), the 21 grammars of decision D, highlighting on a background thread with a cache by content hash, shown blocks upgraded after the paint, small edited blocks inline, 256 KiB cap, labeled plain otherwise | A JS highlighter in the frontend; syntect; highlighting at render time on the preview worker | M2 |
| 0030 | Markdown preview surface | [Accepted](0030-markdown-preview-surface.md): byte-anchored DOM blocks in windows the host sends (HTML only for what the frontend lacks), provisional windows then corrections, progressive placement, large tables and code blocks as pieces, font warm-up | Core-rendered tiles; one DOM node per document; whole-document height estimates | M2 |
| 0031 | Markdown export | [Accepted](0031-markdown-export.md): self-contained HTML from the preview's renderer (images, KaTeX fonts and diagrams inlined; a report of what wasn't carried over); PDF printed by WebView2, refused over a 10 MiB page (user decision 2026-10-16); `krilla` in M6 | `krilla` now (MIT OR Apache-2.0); chunked printing merged with lopdf (MIT) | M2 |
| 0032 | Unlicense allowed | [Accepted](0032-unlicense-allowed.md) (user decision): Unlicense added to the license allowlists for `robust-predicates` in Mermaid's d3; Mermaid pinned at 11.17.2 to keep EPL-2.0 (`elkjs`) out | Allow EPL-2.0 too (Mermaid 12); no Mermaid in v1 | M2 |
| 0033 | Math and diagrams | [Accepted](0033-math-and-diagrams.md): KaTeX bundled, lazy, in 4 ms slices in the page; Mermaid in a sandboxed frame served by the `quire-frame` scheme with its own CSP, SVG shown as an image; errors keep the source with a badge | KaTeX in the core; Mermaid in the page, a worker or a srcdoc frame; inline SVG | M2 |
| 0034 | Patched grammars | [Accepted](0034-patched-grammars.md): tree-sitter-md 0.5.3 and tree-sitter-bash 0.25.1 (`isdigit` on code points, a SEGV) and tree-sitter-sequel 0.3.11 (a leak, upstream's fix) vendored under `third_party/` with scanner fixes found by fuzzing, used through `[patch.crates-io]` | Wait for upstream; drop the two languages; pre-filter code | M2 |
| 0035 | PDF engine | [Accepted](0035-pdf-engine.md): PDFium `chromium/7881` from bblanchon/pdfium-binaries (no V8/XFA), pinned by SHA-256 and licence-checked by `tools/pdfium/fetch-pdfium.ps1`, our own FFI, in workers only; IJG, libpng, libtiff and AGG 2.3 notices approved (user, 2026-10-23) | hayro (Apache/MIT; eager open, no text/forms/save); pdf.js (Apache-2.0); MuPDF (AGPL); Poppler (GPL); our own PDFium build; pdfium-render (MIT/Apache) | M3 (spike) |
| 0036 | PDF worker processes | [Accepted](0036-pdf-worker-processes.md): `quire --pdf-worker` processes, binary frames over pipes (pixels inline), least-loaded dispatch, restart and one retry on a crash or timeout, Job Object; performance cores − 1 | PDFium in the host; a sidecar worker binary; shared-memory tiles; JSON | M3 |

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
