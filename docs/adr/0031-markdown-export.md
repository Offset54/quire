# 0031. Markdown export: self-contained HTML built from the preview's renderer, PDF printed by the webview

- Status: Accepted
- Date: 2026-10-16

## Context

§6.2: Markdown exports to self-contained HTML and PDF in v1 (DOCX in v2). §3.1 lists the same for the Markdown view. §8: every file is hostile, no network. §2: no fake completeness, so an export says what it didn't carry over. M2_PLAN task 11 left the PDF engine to a spike: the webview's print-to-PDF, or a PDF written in the core (with `krilla`) from the block model.

## Decision

**One pipeline for both formats** (`apps/desktop/src-tauri/src/export.rs`, `md.rs` `md_export_html`/`md_export_save`; `apps/desktop/src/md/export.ts`):

1. The core renders the whole document as the preview would (`render_document`: the same sanitizer and image policy, ADR-0028; code highlighted, ADR-0029; footnotes collected at the end as GitHub does) and inlines every image the preview could show as a `data:` URI, with the original bytes (no display-size scaling). Images that can't be inlined keep their alt text and go into the report: remote images while the document's remote toggle is off, files that aren't images, missing files, images over 32 MiB.
2. The frontend parses that HTML into an inert document (`DOMParser`: nothing in it runs or loads), renders math with KaTeX and diagrams in the sandboxed Mermaid frame (ADR-0033; the SVG as an `<img>` data URI, so it stays inert in the export too), drops `data-sourcepos`, and adds the export stylesheet (`export.css`, light and print-friendly) and, when there is math, KaTeX's stylesheet with its woff2 fonts as data URLs. Math and diagram errors go into the report; their source stays in the page.
3. The finished page is a single file with no script and no external reference. **HTML** is written by the host atomically (temporary file, then rename). **PDF**: the host loads the same page from a temporary file in a hidden webview window and prints it with WebView2's `PrintToPdf` (backgrounds on, no header or footer), on a blocking thread, with a 5-minute limit.
4. The report (kind, source, reason) is shown as a notice after the export; under WebDriver `MdController.exportTo(kind, path)` returns it.

**PDF engine for M2: the webview's printer, with a size limit; `krilla` in M6.** The brief already names `krilla` for PDF export of DOCX, PPTX and Markdown in M6 (§5.1, §11 M6); M2 needs PDF now (§6.2), so it prints the export page with the webview. The printer doesn't scale to very large pages (spike below), so **PDF export refuses a page over 10 MiB** (user decision 2026-10-16) with a plain message pointing to HTML export: about 1 MiB of math-heavy Markdown, ~1,100 pages. The frontend checks before asking where to save; the host refuses as well. HTML export has no such limit (up to the 256 MiB document cap).

The two engines compared:

| | WebView2 `PrintToPdf` (chosen) | `krilla` in the core from the block model |
|---|---|---|
| Fidelity | The exported page as the user would see it in a browser: CSS, KaTeX, SVG diagrams, tables, highlighting, fonts with fallback, text shaping for every script | A second layout engine for Markdown: paragraphs, inline runs, tables, lists, code, math (KaTeX produces HTML, not boxes), diagrams (SVG to PDF), font fallback and shaping (rustybuzz) |
| Text in the PDF | Selectable and searchable, fonts embedded | Same, once built |
| Cost | ~200 lines; the HTML export is reused | Weeks: in effect a layout engine, which M4 (DOCX) needs anyway but with Word's rules |
| Platforms | Windows now; macOS `WKWebView.createPDF`, Linux WebKitGTK print-to-file, behind the same command | All platforms alike |
| Pagination control | CSS (`@page`, `break-inside: avoid` on code, tables, figures) | Full |

**Spike** (`tests/ui/export-spike.test.mjs`, release build, raw dev machine with the user's browsers open; slices of the 20 MiB fixture `document-20mib.md` cut at a heading; `bench/results/m2/export-spike-2026-10-15.json`):

| Document | HTML export | PDF export |
|---|---|---|
| UI-flow sample, 48 KB | (flow) | 0.9–1.0 s, 175 KB |
| 1 MiB slice | 4.6 s, 7.6 MB page | 47.5 s, 1,131 pages, 6.8 MB; long tasks up to 3.2 s in the page |
| 4 MiB slice | 29 MB page | not finished: after ~11 min the WebView2 renderer held 5.9 GB and free memory fell under 2 GB (stopped) |
| 20 MiB fixture | ~3 min, 145.5 MB page | not finished: after 8 min the renderer held 12.6 GB, 0.4 GB free (stopped) |

The page is mostly math markup: 31,660 KaTeX expressions (HTML plus MathML) are about 67 MB of the 145.5 MB; the text is 18.7 MB. Chromium lays out and paginates the whole page in one renderer before writing the PDF, so its memory grows with the page. The fixture's 10 image files aren't generated, so its images are all reported "not found" (correct).

## Alternatives

- *PDF from the core with `krilla` (MIT OR Apache-2.0) now*: full control, bounded memory and the same output on every OS, but it means a Markdown layout engine of our own, with math and diagrams converted to PDF drawing: weeks of M6 work pulled into M2. It is the M6 plan.
- *Printing in bounded chunks and merging the PDFs* (lopdf): memory bounded, time linear (~16 min and ~22,000 pages for the 20 MiB fixture), forced page breaks at the seams; a few days for an interim engine. Not chosen (user decision 2026-10-16).
- *Mermaid as inline `<svg>` in the HTML* (M2_PLAN's wording): an inline SVG from a hostile diagram is live markup in the exported page (styles, links, `foreignObject`). As an `<img>` data URI it renders the same and stays inert.
- *Printing the preview itself*: the preview is virtualized (ADR-0030), so most blocks aren't in its DOM, and it carries app chrome.

## Consequences

- An export of a very large document holds the whole page in memory three or four times (core HTML, the parsed document, the page string, the IPC copy): for the 20 MiB fixture's 145 MB page, several hundred MB in the webview. Exports refuse documents over 256 MiB, PDF pages over 10 MiB.
- Math and diagram rendering for the export runs in the page's main thread (KaTeX) and the Mermaid frame, one diagram at a time; parsing and serializing the page are single tasks too. The 1 MiB slice had long tasks up to 2.6 s (HTML) and 3.2 s (PDF): the window doesn't respond meanwhile, against §2's rule for the UI thread. Follow-up (RISKS #17): slice the math loop, show progress with Cancel, and move parsing off the page where possible.
- PDF output depends on the platform webview: on macOS and Linux the same command needs its own print path (M7 cross-platform work).
- DOCX export (v2) won't reuse this pipeline.

## Evidence

- `tests/ui/md-flows.test.mjs`, "export to HTML is self-contained…": the exported page has a `<style>`, no `<script>`, no event handler and no `src`/`href` that leaves the file; the local image is a PNG data URI, the remote tracker is kept out and reported; math is rendered with KaTeX fonts as data URLs; the broken expression and the broken diagram are reported; the diagram is an SVG `<img>`; code is highlighted; the PDF starts with `%PDF-`.
- `export.rs` unit test: local images inline, a non-image and a remote image are reported.
- Spike numbers above (`bench/results/m2/export-spike-2026-10-15.json`).
- `src/md/export.test.ts`: the PDF limit counts UTF-8 bytes.
