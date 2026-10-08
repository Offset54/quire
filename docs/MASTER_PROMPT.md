# Master Prompt: High-Performance Document Viewer & Editor

## 0. Parameters (edit before use)

| Parameter | Value |
|---|---|
| Product name | `[PRODUCT_NAME]` |
| Target platforms | Windows 10/11 x64 first; then macOS (arm64/x64) and Linux x64 |
| Form factor | Offline-first desktop app. A web/WASM build is a stretch goal, so keep the core portable. |
| License policy | Permissive licenses only in the shipped binary (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0). No GPL/AGPL. LibreOffice may be used only as an optional, separately installed external process. |
| Reference machine | 4-core/8-thread laptop CPU (2021-era), 16 GB RAM, NVMe SSD, integrated GPU, 1920×1080 at 150 % scaling |
| UI languages | English, with i18n ready from day one |
| Build order | TXT → Markdown → PDF → DOCX → PPTX → PPT |

---

## 1. Role and mission

You are a principal engineer who has shipped document engines (office suites, PDF viewers, code editors). Design and build **[PRODUCT_NAME]**, a professional desktop application that opens, renders, edits and saves `.txt`, `.md`/`.markdown`, `.docx`, `.pptx`, `.ppt` and `.pdf` files.

Success means three things, in this order:

1. **It never loses or corrupts user data.** Saving is atomic. Content the app doesn't understand is preserved. A crash costs at most about one second of work.
2. **It stays fast at any file size.** The UI never freezes. The first screen appears within budget no matter how large the file is, and the rest streams in progressively.
3. **It renders faithfully.** Documents look the way their authoring application shows them, within tolerances that are measured.

Features come after these three. A smaller feature set that meets all three is better than a larger one that doesn't.

## 2. How you work

- **Plan first.** Your first response is the plan described in §13. Then wait for approval.
- **Persist this brief.** Save this prompt verbatim as `docs/MASTER_PROMPT.md`. Re-read the sections that matter at the start of each milestone.
- **Work in milestones.** Build in the order given in §11. Meet each milestone's Definition of Done (DoD) and report on it before starting the next one.
- **Measure; don't assert.** Every performance claim must come from an automated benchmark run on the §10 fixtures, reported as p50/p95 together with the machine spec. If a budget looks unreachable, stop and bring data and options. Never quietly relax a budget.
- **Spike unknowns.** Before committing to an approach with real uncertainty (IPC tile throughput, IME on canvas surfaces, DOCX layout fidelity), run a time-boxed prototype and record the result in an ADR.
- **No fake completeness.** An unimplemented feature is either absent from the UI or disabled through a capability flag. Don't write stubs that return success, put mock renderers in the shipping path, or leave a `TODO` in any code path a DoD covers.
- **Preserve what you don't understand.** Keep every element, attribute, part or object you can't render or edit, and write it back unchanged. Wherever something can't be rendered, show a visible placeholder rather than a silent gap.
- **Degrade honestly.** If a feature has to degrade above some size (for example, WYSIWYG Markdown above 5 MB, or highlighting on lines over 10 MB), say so in the UI and in the docs. Never degrade silently.
- **Keep the main thread free.** No single task on the UI thread may exceed 8 ms. Any task over 50 ms is a bug.
- **Ship tests with code.** Write unit tests, property-based tests for data structures, integration tests for open/save round trips, and fuzz tests for every parser.
- **Justify dependencies.** Give each one a line in an ADR. Check licenses against §0 automatically (cargo-deny plus a JS license checker) and pin versions.
- **Keep the docs current.** Maintain `docs/PROGRESS.md` (status, benchmarks vs. budget, known gaps), `docs/adr/` and `docs/RISKS.md`. Make small, focused commits.
- **Ask only when the decision is mine.** For scope, licensing and UX trade-offs, ask, and include your recommendation. For everything else, decide, record the decision in an ADR, and move on.

## 3. Scope

v1 is the first public release and v2 is the planned follow-up. Design v1 so that v2 features don't require rewriting the model.

### 3.1 Capability matrix

| Format | View | Edit (v1) | Edit (v2) | Save |
|---|---|---|---|---|
| **TXT** (plus any text-like file: .log .csv .json .xml .yaml, source code) | Virtualized view at any size; encoding detection, line numbers, soft wrap | Full editing at any size: multi-cursor, column selection, regex find/replace (including replace-all on GB-size files), go-to-line, encoding and line-ending conversion | Follow mode for growing logs, syntax highlighting, hex view for binary files | Original encoding, BOM and line endings preserved unless the user changes them; atomic |
| **Markdown** | Source view plus live preview (split or preview-only), outline, CommonMark + GFM (tables, task lists, footnotes, strikethrough, autolinks), YAML front matter, KaTeX math, Mermaid, code highlighting | Source editing with incremental preview, two-way scroll sync, image paste into an assets folder, table helpers | WYSIWYG mode with lossless round trip (minimal source diffs; untouched text is never reformatted) | Untouched regions stay byte-exact |
| **DOCX** (.docx .docm .dotx) | Paginated page view and flow view; styles, numbering, tables, inline and floating images, headers/footers, footnotes/endnotes, sections and columns, comments, tracked changes, fields (cached results), hyperlinks, bookmarks, OMML equations, text boxes | Text and character/paragraph formatting, styles, lists, tables (cell text, rows/columns, merge), images (insert, resize, wrap mode), headers/footers, page setup, comments (add, reply, resolve), accept/reject tracked changes, find/replace across runs | Recording tracked changes, footnote editing, TOC and field update, section editing | Unchanged XML spliced in byte-for-byte; Word opens the file with no repair prompt |
| **PPTX** (.pptx .pptm .potx .ppsx) | Slide view, thumbnail rail, notes, outline; slideshow (fullscreen, basic transitions) with presenter view; master/layout/theme inheritance, every preset geometry, text autofit, tables, charts (from cached data), SmartArt (via its fallback drawing), images including crops, EMF/WMF, media posters, groups | Edit text; move, resize and rotate shapes; insert text box, image, shape or table; delete, duplicate and reorder slides; change layout; speaker notes | Animation playback, chart data editing, master/layout editing | Unchanged parts copied as raw zip entries (no recompression); PowerPoint opens the file with no repair prompt |
| **PPT** (legacy binary) | Imported into the PPTX model (§6.5) | Edited as PPTX after import | Native reader | Saves as .pptx by default; "Save as .ppt" goes only through the converter and shows a fidelity warning |
| **PDF** | Continuous, single and two-up layouts; zoom 10–6400 %; rotation, thumbnails, outline, links, attachment list, layers (OCG), text selection and copy, search, password-protected files | Annotations (highlight, underline, strikeout, note, free text, ink, shapes, stamps), AcroForm filling, page operations (reorder, rotate, delete, insert, extract, merge, split), metadata | True redaction (removes the content itself), adding text and images, limited editing of existing text, OCR for scanned files (optional plugin), signature validation display | Incremental save by default (fast at any size, keeps signatures valid); full rewrite only through an explicit "Save optimized copy" |

**Cross-format:** tabs, recent files, drag-and-drop, file associations (opt-in at install), print, export to PDF (from DOCX, PPTX and MD), a find/replace panel, command palette, undo/redo, autosave with crash recovery, external change detection, light/dark/system themes, and per-monitor DPI.

### 3.2 Non-goals

- Real-time collaboration. Edits are still stored as serializable operations so it can be added later.
- Cloud integrations beyond local and synced folders.
- Executing macros or PDF JavaScript.
- XFA forms.
- `.doc`, `.xls`, `.xlsx` and `.odt`. The provider architecture should still leave room for them.
- Mobile apps.

## 4. Performance budgets

All budgets are measured on the reference machine with a cold OS file cache unless stated otherwise. "First paint" is the first correct pixels of the initial viewport, measured from the file-open command.

| Scenario | Budget |
|---|---|
| App cold start → interactive | ≤ 1.0 s (warm ≤ 0.4 s) |
| Input → visual response (typing, caret, selection) in any editor | p95 ≤ 16 ms, p99 ≤ 33 ms |
| Scrolling or zooming any document | 60 fps, no frame over 50 ms. Placeholders are allowed; a blank screen is not. |
| UI-thread long tasks | None over 50 ms, ever |
| **TXT, 2 GB, mixed line lengths** | First paint ≤ 300 ms. Full line index ≤ 4 s, in the background. Jump to any line ≤ 100 ms once indexed (≤ 300 ms, approximate, before). Literal search ≤ 3 s, with results streaming from the first hit. Insert at any offset ≤ 5 ms. |
| **TXT, one 300 MB line** (minified JSON) | First paint ≤ 300 ms; no hang on caret movement or when toggling wrap |
| **Markdown, 20 MB** | First paint of source and preview ≤ 500 ms; keystroke → preview update ≤ 50 ms |
| **DOCX, 1,000 pages with tables and images** | First page ≤ 1.0 s; full pagination ≤ 10 s in the background; typing p95 ≤ 16 ms mid-document |
| **PPTX, 500 slides, 500 MB of media** | First slide ≤ 1.0 s; slide switch ≤ 100 ms; all thumbnails ≤ 15 s in the background |
| **PDF, 5,000 pages / 1 GB** | First page ≤ 500 ms; a typical page at 100 % ≤ 150 ms; sharp at 800 % zoom ≤ 300 ms. Search streams results; first full pass ≤ 30 s, repeat searches ≤ 2 s. |
| Saving after a small edit | TXT 2 GB: limited only by sequential write speed. DOCX/PPTX: ≤ 2 s whatever the media size. PDF incremental save: ≤ 1 s whatever the file size. |
| Memory | Resident memory beyond the mapped source ≤ 1.5 GB per document (configurable); background tabs shed their caches first |

## 5. Architecture

### 5.1 Stack (default; challenge it only through an ADR backed by evidence)

- **Shell:** Tauri 2, with the web UI in the system webview (WebView2, WKWebView or WebKitGTK).
- **Core:** A Rust workspace. All parsing, layout, rendering, search, editing and saving live here. The frontend never holds a whole document.
- **Frontend:** Strict TypeScript, with React for the app chrome. Hot paths (document surfaces, virtual scrollers, text input) are imperative TypeScript on canvas/DOM and stay outside React's render cycle.
- **Candidate libraries** (verify maintenance status and license when you adopt them):
  - Files and text: `memmap2`, `memchr`, `regex-automata`, `encoding_rs` + `chardetng`, ICU4X (segmentation, UAX #14), `unicode-bidi`
  - ZIP/XML: `zip` (with raw-entry copy), `quick-xml` (streaming pull parser)
  - Shaping and fonts: HarfRust or `rustybuzz`, `skrifa`/`swash`, `fontique` (enumeration and fallback); evaluate `parley` as a base for rich-text layout
  - Rasterization: `tiny-skia` as the CPU baseline; evaluate `vello`/`vello_cpu` behind a feature flag
  - PDF: PDFium through `pdfium-render` (prebuilt binaries); `qpdf` or `lopdf` for structural operations; `krilla` for PDF export
  - Markdown: `comrak` or `pulldown-cmark` (with source positions); tree-sitter for code highlighting; KaTeX and Mermaid in the frontend, loaded lazily
  - Infrastructure: `tracing` (with Chrome-trace export), `criterion`, `proptest`, `cargo-fuzz`, `cargo-deny`

### 5.2 Process and thread model

- **UI process.** The webview plus a thin TypeScript compositor. Its heaviest jobs are drawing ready-made tiles and overlays and handling input.
- **Core.** Rust in the main app process: document models, the job scheduler and the caches.
- **Worker processes** isolate native code that is crash-prone or not thread-safe:
  - PDFium isn't thread-safe. Run a pool of PDFium worker processes (one per physical core minus one, minimum one) for parallel rendering and crash isolation. If a worker crashes, restart it and re-queue its job. The user should see no more than a tile re-rendering.
  - The PPT converter (§6.5) also runs out of process, with a timeout.
- **Job scheduler.** Priority queues, highest first: `Interaction` (edit relayout, hit-testing) → `Visible` → `NearViewport` → `Prefetch` → `Background` (indexing, pagination, thumbnails).
  - Every job carries a cancellation token and a generation number.
  - Scrolling and zooming bump the generation, and stale jobs are dropped before they run.
  - Duplicate requests are coalesced.

### 5.3 Core abstractions (shape, not final API)

```rust
pub trait FormatProvider: Send + Sync {
    fn id(&self) -> FormatId;
    fn probe(&self, head: &[u8], ext_hint: Option<&str>) -> Confidence; // sniff magic bytes, not extensions
    fn open(&self, src: Source, ctx: &OpenCtx) -> Result<Box<dyn Document>>;
}

pub trait Document: Send {
    fn capabilities(&self) -> Capabilities;               // per-feature view/edit/save flags; the UI reads these
    fn info(&self) -> DocInfo;                             // metadata, counts (possibly Estimated)
    fn units(&self) -> Progressive<UnitIndex>;            // pages | slides | line blocks; refines over time
    fn outline(&self) -> Progressive<Outline>;
    fn render(&self, req: ViewRequest, cancel: CancelToken) -> JobHandle<ViewResponse>; // display list or tiles
    fn hit_test(&self, unit: UnitId, pt: Point) -> Option<Position>;
    fn text_stream(&self, from: Position) -> Box<dyn TextStream>; // search and indexing
    fn apply(&mut self, tx: Transaction) -> Result<ChangeSet>;     // returns dirty regions + inverse
    fn save(&self, target: SaveTarget, mode: SaveMode, progress: &Progress) -> Result<SaveReport>;
}
```

Format detection sniffs content:

- A ZIP containing `[Content_Types].xml` is told apart by its main part's content type.
- A CFB file (`D0 CF 11 E0`) with a `PowerPoint Document` stream is a PPT.
- `%PDF-` within the first 1 KB is a PDF.
- Anything else goes through text sniffing.

Mislabeled extensions must still open correctly.

### 5.4 Rendering pipeline

- **Pipeline:** model → layout → display list → raster tiles → compositor.
  - A display list holds positioned glyph runs, paths, images, and clip/transform/opacity groups for each page, slide or line block.
  - Tiles are 256 or 512 px, rendered at the device pixel ratio.
  - The compositor is the frontend canvas.
- **Caching:** Every stage is cached, keyed by (unit, revision, zoom bucket). Edits invalidate dirty rectangles only, so a keystroke re-rasters only the tiles it touches.
- **Zoom:** Transform the existing tiles immediately (blurry), then swap in sharp tiles as they arrive. Keep a low-resolution bitmap of each visible page as the fallback layer.
- **Overlays:** The frontend draws the caret, selection, IME composition, handles, search highlights and annotation editing in a separate layer, using geometry from the core. Overlays never trigger re-rasterization.
- **Huge scroll extents:** Browsers cap element heights at a few tens of millions of pixels. Implement a virtual scroll model with a custom scrollbar so that 50M-line files and 10,000-page PDFs scroll and jump correctly.
- **Text rendering:**
  - DOCX, PPTX and PDF text is shaped and painted by the core, not the webview, so measurement and painting always agree.
  - TXT and MD source views may use virtualized DOM text for the visible lines, which helps IME and accessibility.

### 5.5 Memory

- **Source files.** Memory-map them. Never copy a whole file into the JS heap or into a single Rust buffer.
  - Exception: network drives and cloud placeholder files (such as OneDrive Files On-Demand). Detect them and use a buffered chunk cache (pread + LRU) instead. I/O errors on a mapped view surface as crashes (`EXCEPTION_IN_PAGE_ERROR` / `SIGBUS`).
- **Memory governor.** A global governor holds the byte budget. Every LRU cache (tiles, decoded images, display lists, layouts) registers with it and is evicted by priority.
- **Images.** Decode at display resolution using mip levels. Never decode a 100-megapixel original just to draw a thumbnail.
- **Compact models.** Use arena allocation, intern style and font names, and store each resolved style once, referenced by ID.

### 5.6 IPC

- **Bulk data.** Never send it as JSON or base64. Use Tauri binary responses, Channels, or an async custom URI scheme for tiles and text ranges.
  - Benchmark raw RGBA against fast lossless encodings and choose by measurement.
  - On Windows, evaluate WebView2 shared buffers for zero-copy tiles.
- **Commands.** Keep them small and async. The frontend requests content by viewport. The core answers with what's ready now and follows up with progressive updates.

### 5.7 Editing, undo and recovery

- **Transactions.** Every edit is a transaction of format-specific operations, each with an inverse. Undo/redo coalesces typing and keeps the history within a memory bound.
- **Optimistic edits.** Apply the edit to the model, relayout only the affected unit and repaint only the dirty tiles. Downstream work (re-pagination, re-indexing) runs in the background, incrementally.
- **Crash-recovery journal.** Append each committed transaction to a per-document journal, with fsync batched to at most one per second.
  - After a crash, offer to restore on the next launch.
  - Delete the journal after a clean save or close.

### 5.8 Save pipeline

- **Atomic saves.** Write to a temp file in the same directory, fsync it, atomically replace the original (`ReplaceFileW` on Windows, `rename(2)` elsewhere), then fsync the directory. Preserve file attributes, and ACLs/xattrs where possible.
- **The mapped-file hazard.** Windows can't replace a file that has an open mapping, and on Unix, truncating a mapped file raises `SIGBUS`. Follow this sequence explicitly:
  1. Stream the new content from the old mapping into the temp file.
  2. Copy into memory any undo-history ranges that reference the old mapping.
  3. Unmap the original.
  4. Replace it with the temp file.
  5. Remap the new file and reset the buffer to point at it.
- **External changes.** Detect them with a file watcher plus an mtime/size/hash check on focus. Offer three choices: reload, keep mine, or compare.
- **Save reports.** Each save produces a report listing anything that couldn't be preserved (for same-format saves the list must be empty). Data loss is never silent; the user is told and must confirm.

## 6. Format engineering

### 6.1 TXT and text-like files

- **Buffer.** A piece tree (as in VS Code) over the mapped original plus an append-only add buffer. Each node caches its byte length and newline count, which gives O(log n) offset↔line mapping and O(log n) edits at any size.
- **Line index.** Build it in the background with `memchr`, in 64 KB chunks.
  - Store newline counts per chunk, not per-line offsets, to keep the index small.
  - Until the index is finished, scrolling and go-to-line are proportional to byte offset.
  - The viewport must not jump when exact line numbers arrive.
- **Encodings.**
  - Detection order: BOM → declared encoding → `chardetng` heuristic. The user can override and re-decode.
  - Supported: UTF-8, UTF-16 LE/BE, and legacy code pages (CP949/EUC-KR, Shift_JIS, GB18030, Big5, Windows-125x).
  - Preserve the BOM and line endings (LF, CRLF, CR, or mixed; flag mixed endings in the status bar).
- **Long lines.** Lay out only the visible column window and compute wrapping lazily per visible line. Cap the highlighting and measurement work done per line.
- **Search.**
  - Run `regex-automata` over chunked slices, with overlap at chunk boundaries, in parallel across chunks.
  - Stream results and cap the memory they use.
  - Replace-all runs as one background transaction with progress, undoable in one step.
- **Binary files.** Detect NULs and invalid sequences. Warn before editing, and offer a read-only hex view (v2).

### 6.2 Markdown

- **Source of truth.** The text buffer, using the same engine as §6.1.
- **Incremental parsing.** Re-parse only the blocks an edit touches, widened to the enclosing container (list, block quote, fenced code). Track reference-link definitions globally.
- **Preview.** Render blocks in a virtualized view, using height estimates and a measurement cache. A source map (block ↔ line range) drives two-way scroll sync and click-to-source.
- **Safety.** Sanitize embedded HTML with an allowlist (strip scripts and event handlers). Block remote images by default, with a per-document "load remote content" toggle.
- **KaTeX and Mermaid.**
  - Render each block lazily when it enters the viewport.
  - Run Mermaid in an isolated, offscreen context.
  - If a render fails, show the source with an error badge.
- **Exports.** Self-contained HTML and PDF in v1; DOCX in v2.

### 6.3 DOCX

- **Fast first page.**
  1. Load `[Content_Types].xml`, the rels, styles, numbering, theme, settings and fontTable first.
  2. Stream `document.xml` through a pull parser, emitting blocks progressively.
  3. Lay out and show page 1 while the rest is still parsing.
- **Style resolution.**
  - Order: docDefaults → table style → numbering → paragraph style (following the basedOn chain) → character style → direct formatting.
  - Cache the resolved properties per (style set, direct properties) hash.
- **Layout.** Use our own engine. It must cover:
  - Shaping with a HarfBuzz-class shaper.
  - UAX #14 line breaking with Word-compatible rules: greedy fill (not Knuth–Plass), bidi, and East Asian kinsoku.
  - Tab stops, indents and spacing, including "auto" line spacing and contextual spacing.
  - Widow/orphan control, keep-with-next and keep-lines-together.
  - Page, column and section breaks.
  - Tables: fixed and autofit widths, row splitting, repeated header rows, nested tables.
  - Floating objects with all wrap modes.
  - Footnotes that reduce the available page height.
  - Headers and footers per section (first, even and odd).
- **Incremental pagination.**
  - Cache each paragraph's layout, keyed by (content revision, available width, resolved style).
  - After an edit, re-lay out the paragraph, then re-paginate forward from the affected page.
  - Stop as soon as page boundaries line up with the previous layout again.
- **Fonts.**
  - Use system fonts plus bundled metric-compatible substitutes: Carlito for Calibri, Caladea for Cambria, Liberation Sans/Serif/Mono for Arial, Times New Roman and Courier New, and Noto CJK for CJK fallback.
  - Deobfuscate embedded fonts (`.odttf`).
  - Show a "fonts substituted" indicator that lists every substitution.
- **Round-trip fidelity.**
  - Keep each block's original XML byte range.
  - On save, splice unchanged ranges back byte-for-byte and re-serialize only the blocks that changed.
  - Keep the root element and its namespace declarations intact.
  - Preserve `mc:AlternateContent`, unknown elements, custom XML, SDTs, rsids and `vbaProject.bin` (macros are never executed).
  - Copy unchanged parts as raw zip entries.
- **Model.** Tracked changes and comments are first-class in the model, never flattened.
- **Fields and equations.** Fields show their cached results. OMML equations go through an equation layout module; if one can't be laid out, show a placeholder and keep the XML.

### 6.4 PPTX

- **Opening.** Load `presentation.xml`, the rels, masters, layouts and theme first. Load slides and media lazily, on demand. Render thumbnails in the background at low resolution through the same display-list pipeline.
- **Inheritance.**
  - Resolve placeholder properties slide → layout → master.
  - Take text styles from the master's `txStyles` and the presentation's `defaultTextStyle`.
  - Apply theme colors with every transform (lumMod, lumOff, tint, shade, satMod, alpha and so on) and honor color-map overrides.
- **Geometry.**
  - Evaluate every preset shape from the ECMA-376 `presetShapeDefinitions` guide formulas, and support custom geometry.
  - Lines: styles and arrowheads.
  - Fills: solid, gradient, pattern and picture, including picture crop (`srcRect`) and tiling.
  - Effects (outer shadow, glow, soft edges) are approximated. 3D renders flat but is preserved.
- **Text.** Honor `bodyPr` insets, anchoring, vertical text, columns, `normAutofit` (fontScale + lnSpcReduction) and `spAutoFit`. Support bullets (character, auto-numbered and picture).
- **Charts and embedded objects.**
  - Charts: render the common types (bar/column, line, area, pie/doughnut, scatter, combo) from the cached values in the chart part. For any other type, use its fallback image if there is one; otherwise show a labeled placeholder.
  - SmartArt renders from its cached drawing part.
  - OLE objects render from their preview image.
- **EMF/WMF/EMF+.** Write a converter that turns records into display-list items, covering the common records. Skip and log unknown records.
- **Media.** Show poster frames. Play audio and video through the webview's media element wherever the codec is supported.
- **Slideshow.**
  - Fullscreen, with keyboard and clicker navigation.
  - Basic transitions (fade, push, wipe); any other transition falls back to a cut.
  - Presenter view on a second display: current and next slide, notes, timer.
- **Saving.** Same splice-and-raw-copy strategy as DOCX. Only modified slide parts and relationships are rewritten; media entries are copied without decompressing.

### 6.5 PPT (legacy binary)

- **Default import path.**
  - Use an optional external converter: `soffice --headless --convert-to pptx`.
  - Run it out of process, in a temporary sandbox directory, with a timeout.
  - Open the result with the PPTX engine.
  - If the converter isn't installed, detect that and guide the user. Never bundle it in a way that breaks §0.
- **Fallback without a converter.** Offer a quick look: extract per-slide text from the `PowerPoint Document` stream and show the embedded pictures from the `Pictures` stream.
- **Saving.** Mark the document "Imported from .ppt". Saves default to .pptx. "Save as .ppt" goes back through the converter with an explicit fidelity warning.
- **v2 option** (needs an ADR): a native read-only parser over CFB/OLE2 (the [MS-PPT] spec) covering the common record set.

### 6.6 PDF

- **Loading.**
  - Open documents through PDFium custom file-access callbacks over the mapped or chunked source. Never copy the whole file into memory.
  - Use the data-availability (linearization) APIs to show the first page progressively.
  - Tolerate broken xref tables (PDFium repairs them) and show a "repaired" banner.
- **Rendering.** Render tiles per §5.4. Page content, annotations and form fields go in separate layers, so editing an annotation never re-renders the page. Respect OCG (layer) visibility.
- **Text.**
  - Extract each page's text, with character boxes, lazily, and cache it for selection, copy and search.
  - A background indexer runs across the worker pool, with progress.
  - Selection order must be correct for RTL and CJK text.
- **Annotations and forms.** Use PDFium's annotation and form-fill APIs. Generate appearance streams for new and edited annotations so other viewers display them.
- **Page operations.** Use `qpdf`/`lopdf`, or PDFium's import/move APIs where they're enough. Merging and splitting keep outlines and links where possible.
- **Saving.**
  - By default, append an incremental update to the original (PDFium `FPDF_INCREMENTAL` or equivalent).
  - A full optimized rewrite happens only through the explicit "Save optimized copy" command.
  - Warn before any edit that would invalidate a digital signature.
- **Security.** JavaScript, launch actions, external URIs and opening embedded files are either disabled or need explicit user confirmation.
- **v2.**
  - True redaction removes the underlying content (text, images and vector paths, not just a black box). Verify it by extracting the text afterwards.
  - An OCR plugin adds an invisible text layer.
  - Signature validation is shown in the UI.

## 7. UX requirements

- **Professional and calm.**
  - Dense but readable, with consistent iconography.
  - Menus and shortcuts that feel native on each OS (Ctrl on Windows/Linux, Cmd on macOS).
  - A command palette and a contextual toolbar for each format. No ribbon is required.
- **Layout.**
  - Tabs across the top.
  - A left rail for thumbnails, outline and search results.
  - The document in the center.
  - A collapsible right-hand inspector for formatting and properties.
  - A status bar showing page/slide/line, zoom, encoding, line endings, word count, save state and background-job progress.
- **Progressive everything.**
  - Show whatever is ready and label estimates as estimates ("~1,240 pages").
  - Indexing and pagination show non-modal progress with a cancel option.
  - Opening a file never shows a modal spinner.
- **Opening files.**
  - Drag-and-drop, recent files with thumbnails, and reopening the last session.
  - "Open as plain text" is available for any file.
  - Unsupported or encrypted files get a clear message; prompt for a password where the format supports it.
- **Zoom.** Ctrl+wheel and pinch, fit width, fit page and preset levels, all smooth.
- **Themes.** Light, dark and system. In dark mode the document canvas can stay light (user toggle).
- **Settings.**
  - Performance: memory budget, worker count.
  - Font substitution table.
  - Per-format defaults.
- **Performance HUD** (developer toggle): frame times, queue depths, cache sizes and the last N jobs.

## 8. Robustness, security and privacy

- **Assume every file is hostile.** Parsers return errors; they never panic. Isolate panics at the provider boundary with `catch_unwind`, and keep native libraries in worker processes.
- **ZIP.** Limit entry count, total decompressed size and compression ratio (zip bombs). Reject entries with path traversal.
- **XML.** No DTDs or external entities (XXE). Limit entity expansion and nesting depth.
- **Corrupt files.** Open them on a best-effort basis with repair, and show a banner listing what was skipped. One broken part must never stop the rest of the document from being viewed.
- **Network and telemetry.** No network access by default and no telemetry. Remote content (Markdown images, linked DOCX images, PDF URIs) loads only after an explicit user action.
- **Webview hardening.** A strict Content Security Policy, no remote scripts, and minimal Tauri capability permissions.
- **Macro-enabled files.** They open with macros preserved but never executed, and the user sees a notice.

## 9. Internationalization and accessibility

- **Unicode.** Full support, including complex-script shaping, bidi, vertical text (PPTX/DOCX), CJK line breaking, combining marks and color emoji.
- **IME.**
  - Korean, Japanese and Chinese composition must work in every editable surface, including canvas-based ones.
  - No characters may be duplicated or dropped.
  - Composition shows inline at the caret, and the candidate window is positioned correctly.
  - Test with Microsoft IMEs on Windows and the native IMEs on macOS.
- **Paths.** Handle Unicode paths, Windows paths longer than 260 characters, OneDrive-synced folders and network shares.
- **Accessibility.**
  - Every feature works from the keyboard.
  - Screen readers can read document text (through a hidden, virtualized accessible text layer for canvas surfaces) and every UI control.
  - Support high-contrast mode, and respect OS text scaling and reduced-motion settings.
- **Strings.** Externalize UI strings from day one, using ICU MessageFormat.

## 10. Testing, fidelity and benchmarking

- **Fixture generators.** Scripts in the repo, with deterministic seeds, produce a fixture for every row in §4: a 2 GB mixed TXT, a 300 MB single-line JSON, a 20 MB Markdown file, a 1,000-page DOCX, a 500-slide PPTX with about 500 MB of media, a 5,000-page PDF (linearized and not) and a 1,000-page scanned PDF.
- **Real-world corpora.**
  - CommonMark and GFM spec tests.
  - The pdf.js and PDFium test PDFs.
  - Apache POI and LibreOffice test documents.
  - A curated set of 100+ real DOCX/PPTX files: Office-authored, Google Docs exports, LibreOffice exports, CJK and RTL documents, templates, and table-heavy files.
  - Track corpus licenses. Don't commit files that can't be redistributed; fetch them in CI instead.
- **Visual fidelity.**
  - Reference renders are PDF exports from Microsoft Office where available, otherwise from LibreOffice.
  - Compare each page of our render with SSIM plus a perceptual diff, and keep a per-file dashboard.
  - CI fails on any regression beyond the threshold.
  - Set the initial targets after measuring a baseline in M4/M5, then only raise them.
- **Round trips.**
  - Open → save with no edits must leave every unmodified part byte-identical.
  - OOXML output must pass the Open XML SDK's `OpenXmlValidator` (run from a small .NET CLI in CI).
  - PDFs must pass `qpdf --check`, and untouched pages must re-render identically.
  - Scripted edits (insert, delete, format, table operations) are followed by re-opening the file and verifying the result.
- **Property tests.**
  - The piece tree matches a naive string model.
  - Pagination is deterministic.
  - Undo is an exact inverse: apply then undo is the identity, byte-exact on save.
- **Fuzzing.**
  - Write a `cargo-fuzz` target for every parser: ZIP/OOXML, XML, Markdown, encoding detection, EMF/WMF and CFB.
  - Run them nightly. Every crash becomes a regression test.
- **Performance.**
  - Run `criterion` micro-benchmarks, plus end-to-end scenario benchmarks that drive the real app through WebDriver and record the §4 metrics.
  - Commit the results as JSON and chart them.
  - CI fails on any regression over 10 %.
- **Manual QA checklist** for each milestone: IME, HiDPI, multiple monitors, dark mode, and low memory (simulate 8 GB).

## 11. Milestones and Definitions of Done

**M0 Foundations.**
- Scope:
  - Workspace setup, CI (Windows first, then macOS and Linux), and lint, format and license gates.
  - The Tauri shell.
  - Job scheduler with cancellation, and the memory governor.
  - The binary IPC path.
  - A virtual scroller with a custom scrollbar.
  - `tracing` and the performance HUD.
  - Fixture generators and the benchmark harness.
- DoD:
  - The empty app starts within budget.
  - A synthetic 10M-row list scrolls at 60 fps.
  - The core → screen tile round trip has been measured and documented in an ADR.

**M1 TXT engine.**
- Scope:
  - Piece tree, mapped/chunked sources and the background line index.
  - Encodings.
  - The viewer and editor, including multi-cursor and search/replace.
  - Atomic save using the mapped-file sequence.
  - Journal recovery and external change detection.
- DoD:
  - Every TXT row in §4 is met.
  - Property and fuzz tests pass.
  - Killing the process mid-edit loses at most one second of work.
  - Untouched content survives save byte-for-byte.

**M2 Markdown.**
- Scope: incremental parsing, the virtualized preview, scroll sync, lazy math and Mermaid, sanitization, and exports.
- DoD:
  - At least 99 % of the CommonMark and GFM spec tests pass, with the exceptions listed.
  - The Markdown row in §4 is met.

**M3 PDF.**
- Scope:
  - Worker pool and progressive loading.
  - Tiles, thumbnails, outline, text layer and search.
  - Annotations, forms and page operations.
  - Incremental save.
- DoD:
  - The PDF rows in §4 are met.
  - The whole corpus renders without an app crash, and recovery from a worker crash has been verified.
  - Annotations display correctly in Acrobat, Edge and Chrome.
  - Incremental save leaves existing signatures valid.

**M4 DOCX.**
- Scope:
  - 4a (viewing): model, style resolution, layout, pagination (flow view first, then page view) and fonts.
  - 4b (editing): editing and splice save.
- DoD:
  - The DOCX rows in §4 are met.
  - The fidelity baseline is set and met.
  - The validator passes on the whole corpus.
  - Word opens 30 checklist files, edited by us, with no repair prompt.

**M5 PPTX.**
- Scope: viewing, then editing, then the slideshow and presenter view.
- DoD:
  - The PPTX rows in §4 are met.
  - The fidelity baseline is set and met.
  - Round-trip tests pass.
  - PowerPoint opens edited files with no repair prompt.

**M6 PPT, export and print.**
- Scope:
  - Converter integration and the text-and-pictures fallback.
  - Export to PDF (through `krilla`) for DOCX, PPTX and MD.
  - Printing.
- DoD: A .ppt corpus opens through the converter, and the fallback works without it. Exported PDFs pass `qpdf --check` and match on-screen rendering within the fidelity thresholds.

**M7 Hardening and release.**
- Scope:
  - Accessibility audit and the IME test matrix.
  - Fuzz soak: at least 24 hours per target with no new crashes.
  - Leak soak: open and close 1,000 files with flat memory.
  - Signed installers: MSI or NSIS on Windows, a notarized DMG on macOS, AppImage/deb on Linux.
  - Signed auto-update and opt-in file associations.
  - Local, opt-in crash reports.
- DoD: All earlier DoDs still hold, and the release checklist is complete.

## 12. Repository layout (suggested)

```
apps/desktop           Tauri app (Rust host + TS frontend)
crates/core            scheduler, memory governor, caches, IPC types, provider traits
crates/text            piece tree, line index, encodings, search
crates/markdown
crates/ooxml-common    zip/rels/content types, splice writer, DrawingML, themes, fonts
crates/docx
crates/pptx
crates/legacy-ppt
crates/pdf             PDFium worker protocol + structural ops
crates/layout          shaping, line breaking, paragraph/table layout
crates/raster          display list + rasterizer
crates/emf
workers/pdfium-worker
tools/fixtures         deterministic fixture generators
tools/fidelity         visual-diff harness + dashboard
bench/  fuzz/
docs/                  MASTER_PROMPT.md, ARCHITECTURE.md, adr/, PROGRESS.md, RISKS.md
```

## 13. Your first response

Don't write application code yet. Reply with:

1. **Assumptions and open questions.** Include only those that change the plan, each with your recommendation.
2. **A draft `ARCHITECTURE.md`**, two pages at most, with:
   - one component diagram;
   - a data-flow diagram for scrolling to page 4,000 of a 1 GB PDF;
   - a data-flow diagram for typing a character on page 600 of a DOCX.
3. **An ADR list.** One line per major decision (stack, PDF engine, layout engine, IPC transport, buffer structure, PPT strategy), with the alternatives considered and the license of each.
4. **A risk register.** The top 10 risks (for example, DOCX layout fidelity, WebKitGTK performance, PDFium threading, IME on canvas surfaces), each with likelihood, impact, mitigation and the earliest milestone that retires it.
5. **The M0 task breakdown**, with estimates.

Then stop and wait for my go-ahead.
