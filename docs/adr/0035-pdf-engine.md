# 0035. PDF engine: PDFium, pinned prebuilt binaries, our own FFI

- Status: Accepted (licences: user decision, 2026-10-23)
- Date: 2026-10-23
- Replaces the proposed index entry 0011.

## Context

M3 (§11) needs rendering at any zoom, text with character boxes, search, annotations with appearance streams, AcroForm filling, page import, and an incremental save that keeps signatures valid, on a 5,000-page / 1 GB file within the §4 PDF rows. §5.1 names PDFium through `pdfium-render` with prebuilt binaries; §5.2 puts PDFium in worker processes. §0 allows only permissive licences in the shipped binary.

The engine spike of 2026-10-22 (`spikes/pdf-engine`, [M3 plan](../M3_PLAN.md), `bench/results/m3/pdf-engine-spike-2026-10-22.json`) measured PDFium (bblanchon/pdfium-binaries `chromium/7881`) and hayro 0.8 on the §10 fixtures.

## Decision

1. **PDFium** is the engine, in worker processes only (ADR-0036). The host never loads it.
2. **Binaries:** bblanchon/pdfium-binaries, release `chromium/7881` (PDFium 151.0.7881.0), the builds without V8 and XFA, for windows-x64, windows-arm64, macos-arm64, macos-x64 and linux-x64. `tools/pdfium/pdfium.json` pins each archive by SHA-256; `tools/pdfium/fetch-pdfium.ps1` downloads into `.cache/pdfium/<version>/<platform>`, and fails unless the hash matches, `args.gn` says `pdf_enable_v8 = false`, `pdf_enable_xfa = false`, `is_debug = false`, and the `licenses/` folder holds exactly the reviewed files. It writes `THIRD_PARTY_NOTICES.txt` (every licence text) to ship next to the library. CI runs it on every OS: that is the licence gate for the native binary, which cargo-deny can't see.
3. **Our own FFI.** The worker declares the subset of the C API it uses (`workers/pdfium-worker`), loaded with `libloading` (ISC). `pdfium-render` 0.9.4 (MIT/Apache-2.0) supports the API up to 7881 but keeps the raw bindings private, and tile rendering needs `FPDF_RenderPageBitmapWithMatrix` with a clip; the worker calls PDFium from one thread, so the wrapper's locking buys nothing.
4. **Licences in the binary**, all permissive:

| File in `licenses/` | Licence | Approval |
|---|---|---|
| pdfium.txt | BSD-3-Clause | §0 |
| abseil.txt | Apache-2.0 | §0 |
| simdutf.txt | Apache-2.0 OR MIT | §0 |
| llvm-libc.txt | Apache-2.0 WITH LLVM-exception | §0 |
| fast_float.txt | Apache-2.0 OR MIT OR BSL-1.0 | §0 (BSL-1.0 approved 2026-10-08) |
| lcms.txt | MIT | §0 |
| libopenjpeg.txt | BSD-2-Clause | §0 |
| zlib.txt | Zlib | §0 |
| freetype.txt | FTL (FreeType's own; it is dual FTL / GPL-2.0, we take FTL) | user, 2026-10-08 |
| icu.txt | Unicode-3.0 | user, 2026-10-08 |
| libjpeg_turbo.ijg, libjpeg_turbo.md | IJG; BSD-3-Clause; Zlib | **user, 2026-10-23** |
| libpng.txt | libpng-2.0 | **user, 2026-10-23** |
| libtiff.txt | libtiff | **user, 2026-10-23** |
| agg23.txt | Anti-Grain Geometry 2.3 notice | **user, 2026-10-23** |

The four approved on 2026-10-23 are notice licences: use, modification and redistribution are allowed if the copyright notice and permission text travel with the binary, which the notices file does. None is copyleft. Any new file in a future build's `licenses/` fails the fetch until it is reviewed here.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| hayro 0.8 (pure Rust) | Apache-2.0 OR MIT | Fast once open (17–27 ms per page) but `Pdf::new` walks every page: 1.3–1.8 s cold on the 1 GB fixture before page 1 (budget 500 ms). No text extraction, forms, annotations or saving, all M3 scope. Stays a candidate for a lazy object reader or an independent test renderer. |
| pdf.js | Apache-2.0 | Puts the document in the JS heap (§5.5); slow at 1 GB |
| MuPDF | AGPL-3.0 | Excluded by §0 |
| Poppler | GPL-2.0 | Excluded by §0 |
| PDFium built by us | as above | Same licences (JPEG and PNG decoding can't be dropped); a Chromium-style build (gn, depot_tools) on three OSes. Kept as the fallback if the binary project stalls, and an M7 hardening option. |
| `pdfium-render` | MIT OR Apache-2.0 | Raw bindings private in 0.9; tile rendering needs them (above) |

## Consequences

- `pdfium.dll` (7.2 MB on Windows) ships next to the worker; the host binary and its warm start are unaffected (RISKS #15).
- Upgrading PDFium means new hashes, a re-review of `licenses/`, and re-running the engine checks (the save read-elision guard of ADR-0041 depends on PDFium's copy pattern).
- PDFium bugs crash a worker, not the app (ADR-0036); PDFium is fuzzed upstream (OSS-Fuzz).
- On Windows, custom file access takes a 32-bit length (`unsigned long`): files of 4 GiB or more are opened by path in the worker (ADR-0037).

## Evidence

Spike, raw dev machine (Core Ultra 7 258V, 32 GB, NVMe), 2026-10-22:
- Fresh worker process to page 1 rendered at 100 %, cold file: 39–56 ms (bind 5–8 ms, open 1–5 ms, render 17–33 ms).
- 512 px tile at 800 %: 1.3–3.5 ms (book), 21 ms (scanned).
- Text with character boxes, 5,000 pages: 10.0 s in one worker, 3.0 s in three.
- Incremental save with the copy reads elided: 119–160 ms cold.
- hayro: open 1.3–1.8 s cold; no text, forms, annotations or save.
