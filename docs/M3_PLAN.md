# M3 PDF: task breakdown

Estimates are in focused engineer-days for one engineer, and include tests and docs for each task. They'll be re-estimated from actuals at the M3 report.

## Scope and DoD (§11)

Scope: worker pool and progressive loading; tiles, thumbnails, outline, text layer and search; annotations, forms and page operations; incremental save.

From the capability matrix (§3.1), v1:
- **View:** continuous, single and two-up layouts; zoom 10–6400 %; rotation, thumbnails, outline, links, attachment list, layers (OCG), text selection and copy, search, password-protected files.
- **Edit:** annotations (highlight, underline, strikeout, note, free text, ink, shapes, stamps), AcroForm filling, page operations (reorder, rotate, delete, insert, extract, merge, split), metadata.
- **Save:** incremental save by default (fast at any size, keeps signatures valid); a full rewrite only through an explicit "Save optimized copy".
- Not in v1: true redaction, adding text and images, editing existing text, OCR, signature validation display (all v2). Not ever: running PDF JavaScript, XFA forms (§3.2).

DoD:
- The PDF rows in §4 are met, measured in ref-sim against `bench/budgets.json` on the 5,000-page / 1 GB fixture (linearized and not): first page ≤ 500 ms; a typical page at 100 % ≤ 150 ms; sharp at 800 % ≤ 300 ms; search streams results, first full pass ≤ 30 s, repeat searches ≤ 2 s; incremental save ≤ 1 s whatever the file size.
- The whole corpus renders without an app crash, and recovery from a worker crash has been verified.
- Annotations display correctly in Acrobat, Edge and Chrome.
- Incremental save leaves existing signatures valid.
- The cross-cutting rows hold for PDF too: 60 fps scrolling and zooming with no frame over 50 ms (placeholders allowed, never blank; platform stalls reported apart, ADR-0024), no UI-thread task over 50 ms, input → visual response p95 ≤ 16 ms / p99 ≤ 33 ms in form fields and while dragging a selection or an annotation, and resident memory beyond the mapped source ≤ 1.5 GB per document **including the worker processes**.

§6.6 governs the design: PDFium through custom file access over the mapped or chunked source, never a whole-file copy; linearization / data-availability APIs for a progressive first page; repaired xref tables with a "repaired" banner; tiles per §5.4 with page content, annotations and form fields in separate layers; OCG visibility respected; lazy per-page text with character boxes, a background indexer across the worker pool, correct selection order for RTL and CJK; PDFium's annotation and form-fill APIs with generated appearance streams; incremental save by default, a warning before any edit that would invalidate a signature; JavaScript, launch actions, external URIs and embedded files disabled or behind explicit confirmation. §5.2: a pool of PDFium worker processes (physical cores minus one, at least one), restarted on a crash with the job re-queued.

Carried into M3: PDF tiles in the webview report through `memory_report` (`src-tauri/src/frontend_cache.rs`, done in M2 task 15); RISKS #6 (PDFium process model) retires here; the hybrid-CPU watch item ("physical cores minus one" on P/E machines) is decided here.

## The engine spike (2026-10-22)

`spikes/pdf-engine` (not shipped) measured PDFium against hayro on the §10 fixtures, raw on the dev machine (Core Ultra 7 258V: 4 P + 4 LP E cores; not ref-sim). PDFium is bblanchon/pdfium-binaries `chromium/7881` (PDFium 151.0.7881.0, Windows x64, no V8 or XFA; `pdfium.dll` 7.2 MB), called through a thin hand-written FFI. "Cold" means after `refsim purge` of the file. Results: [`bench/results/m3/pdf-engine-spike-2026-10-22.json`](../bench/results/m3/pdf-engine-spike-2026-10-22.json); fetch the binary with `spikes/pdf-engine/fetch-pdfium.ps1`.

| Measurement | PDFium | hayro 0.8 |
|---|---|---|
| New worker process → page 1 rendered at 100 %, cold file | **39–56 ms** (spawn, bind 5–8 ms, open 1–5 ms, render 17–33 ms) | n/a (in-process): open alone **1.3–1.8 s** cold, because `Pdf::new` walks every page |
| Jump to a far page never visited, cold file | **260–920 ms** before PDFium has walked the page tree; **16–28 ms** after it has | 17–27 ms (after its eager walk) |
| Page tree walk, all 5,000 pages, cold | 1.3–2.1 s (serial, in the worker) | part of open |
| Page at 100 % (1190 × 1684 px), warm | 12–29 ms | 17–27 ms |
| 512 px tile at 800 % | 1.3–3.5 ms (book); 21 ms (scanned: the whole 200 dpi JPEG is decoded per tile) | no tile entry point in the simple API |
| Full text pass with character boxes, 5,000 pages (12.5 M chars) | 1 worker 10.0 s; 3 workers 3.0 s; 4: 2.6 s; 7: 2.1 s | no text extraction |
| Worker memory (peak private bytes) | 18–28 MiB rendering; **grows ~0.2 MiB per page touched** (one document, text of all 5,000 pages: 1,021 MiB); reopening every 250 pages: 57–65 MiB | 30 MiB |
| Incremental save, one note annotation on page 2,500 | **1.4–2.1 s cold, 0.46 s warm**: PDFium streams the whole original back through the write callback; the update itself is 698 KB. With the copy reads answered without touching the file (below): **119–160 ms cold** | no saving |
| Annotations, forms, page import, incremental save | all in the API (471 exports) | none |

What the spike settles:
- **Engine: PDFium.** hayro is fast once open, but its eager open misses the first-page budget on the 1 GB fixture and it has no text extraction, forms, annotations or saving, all M3 scope. pdf.js would put the document in the JS heap; MuPDF and Poppler are excluded by §0. Recorded in ADR-0035; the licences inside `pdfium.dll` are decision A below.
- **Our own FFI, not `pdfium-render`.** pdfium-render 0.9.4 (MIT/Apache-2.0) supports the API up to `chromium/7881` but hides the raw bindings that tile rendering (`FPDF_RenderPageBitmapWithMatrix` with a clip) needs; the worker needs only a subset of the C API, called from one thread per process.
- **Worker processes cost little.** A fresh worker reaches page 1 in about 50 ms, so workers start when the first PDF opens, not at app start (warm start is untouched; RISKS #15). `pdfium.dll` is loaded only by workers: the host binary doesn't grow by it.
- **Far-page jumps need the page tree walked early.** PDFium finds page N by loading the page dictionaries before it, scattered across the 1 GB file. Plan: walk the tree in the background right after open, prefetching the page objects' byte ranges in parallel (the xref gives their offsets), and measure whether jumps then meet the 100 ms effective budget; jumps before the walk finishes are reported separately.
- **Worker memory must be governed.** PDFium keeps every object it has parsed until the document closes. The indexer reopens its document every 250 pages; render workers reopen theirs when their private bytes pass a cap; worker private bytes count against the governor.
- **PDFium's incremental save can meet the budget** with one change: while `FPDF_SaveAsCopy(FPDF_INCREMENTAL)` runs, reads that continue its sequential copy of the original (which we discard; we keep only bytes past the original's length) are answered with zeros instead of from the file. Every other read is served for real (none happened on the fixtures). The resulting update was identical to the real one but for the trailer `/ID`, which PDFium randomizes on every save. The update is then appended to the file in place (decision B). A guard reopens original + update in a worker and checks it before appending; CI compares elided and real updates (minus `/ID`) over the corpus, because the elision depends on PDFium's read pattern.
- Windows allows appending to a file we hold mapped (700 KB + fsync in 3.8 ms); truncating it is refused (os error 1224), so recovery truncates before mapping.
- `FPDF_FILEACCESS.m_FileLen` is an `unsigned long`, 32 bits on Windows: custom file access can't open files of 4 GiB or more there. Such files are opened by path in the worker (PDFium's own 64-bit file stream; crash isolation covers I/O faults), and labeled.

## Effective budgets in ref-sim

ADR-0021 derates `cpu` work by 1.5 and `mixed` work (cold I/O plus CPU) by `max(1.5, D_io)`, `D_io` measured per run (it stayed under 1.5 in the M2 runs, so the table uses 1.5).

| §4 row | Budget | Kind | Effective here |
|---|---|---|---|
| PDF 5,000 pages / 1 GB: first page (cold file) | 500 ms | mixed | ≈ 333 ms |
| A typical page at 100 % (cold region) | 150 ms | mixed | ≈ 100 ms |
| Sharp at 800 % zoom | 300 ms | cpu | 200 ms |
| Search: first full pass | 30 s | mixed | ≈ 20 s |
| Search: repeat | 2 s | cpu | 1.33 s |
| Incremental save after a small edit | 1 s | mixed | ≈ 667 ms |
| Scrolling and zooming | 60 fps, no frame over 50 ms | raw | p99 frame ≤ 16.7 ms, max ≤ 50 ms, without platform stalls (ADR-0024) |
| Input → visual response (form typing, selection drag, annotation drag) | p95 16 ms, p99 33 ms | cpu | p95 10.7 ms, p99 22 ms |
| UI-thread long tasks | none over 50 ms | zero | 0 |
| Memory beyond the mapped source, host + workers | 1.5 GB | raw | 1.5 GB |

Our readings, to be recorded in `bench/budgets.json`:
- **First page** = file-open command → the first viewport (fit width, 1280 × 720 logical at 150 %) shows page 1 with sharp tiles. No worker is pre-started; the cold run includes spawning one. Both the linearized and the plain fixture.
- **Typical page at 100 %** = go-to-page → the viewport shows that page sharp at 100 %, for pages never rendered in the session in a cold file region (pages 1,000, 2,500, 3,999, 4,998). Reported twice: right after open, and after the background page walk; the row is judged on the second, and the first is reported until the walk itself is shown to be fast enough to judge both (risk below).
- **Sharp at 800 %** = zoom command from 100 % to 800 % → every visible tile sharp (scaled tiles are allowed in between, §5.4). On the book and the scanned fixture.
- **Search** = the fixture's `token-NNNNN` per page: time to the first result, the full pass while results stream, then a repeat search for another term from the text cache.
- **Incremental save** = one note annotation on page 2,500 → save command → the update is appended and flushed (fsync) and the status bar says saved, with the file cache cold.

## Tasks

| # | Task | Output | Est. (d) | Depends on |
|---|---|---|---|---|
| 1 | PDFium binaries | Pinned bblanchon/pdfium-binaries release per platform (Windows x64 first; macOS and Linux in CI), fetched by a script with SHA-256s committed; bundled next to the worker as a Tauri resource; a third-party notices file with every licence inside the binary; the licence gate extended to the native binary (a manifest checked in CI, since cargo-deny can't see it); **ADR-0035** (engine, binaries, own FFI) | 2 | decision A |
| 2 | Worker process and protocol | `workers/pdfium-worker` (small exe, own FFI over the C API) and `crates/pdf` host side: spawn on first PDF, handshake with version check, a length-prefixed binary protocol over pipes, tiles rendered into a shared memory section (try the WebView2 SharedBuffer's own file mapping, zero copies), jobs from the scheduler with priorities, generations and cancel; crash detection, restart and re-queue; Job Object (kill on close, memory limit) and a lowered integrity level; pool size from physical cores minus one, efficiency cores not counted (hybrid CPUs), at least one; one editor worker owns mutable state (RISKS #6); worker private bytes reported to the governor, recycle over a cap; **ADR-0036** | 6 | 1 |
| 3 | Opening and progressive loading | `%PDF-` within the first 1 KB as the format probe (§5.3); custom file access over the mapped source (guarded copies, ADR-0007) or the chunked one for network and cloud files, with `FPDFAvail_*` so a linearized file shows page 1 before the rest arrives; by-path open over 4 GiB; a virtual file (original + in-memory updates) that every worker opens, which carries unsaved edits and OCG view state; password prompt (§7) and wrong-password retry; "repaired" banner from `FPDF_DocumentHasValidCrossReferenceTable`; page index: sizes known for visited pages, the rest estimated ("~"), refined by the background page walk with parallel prefetch of page objects; **ADR-0037** | 5 | 2 |
| 4 | Tiles and layers | A `TileSource` for PDF layouts over `crates/core` (continuous, single, two-up; rotation; zoom 10–6400 % in zoom buckets; device-pixel-ratio aware); a low-resolution bitmap per visible page as the fallback layer, scaled tiles on zoom then sharp swaps; page content rendered without annotations, annotations and form widgets in their own transparent layer (spike first: content-only render plus `FPDF_FFLDraw` on a transparent bitmap, and which annotation subtypes it draws); tile caches in the host governor and the webview's through `memory_report`; **ADR-0038** | 7 | 2, 3 |
| 5 | PDF surface | The PDF view on the `DocumentSurface` compositor and virtual scroller (ADR-0005): page frames and placeholders (never blank), Ctrl+wheel and pinch zoom around the pointer, fit width, fit page, presets, rotation, layout switch, keyboard navigation, go-to-page, status bar page n / ~N and zoom; HUD rows for worker queues | 5 | 4 |
| 6 | Navigation panes | Thumbnails (Background priority, low resolution, governed cache, virtualized rail); outline (bookmarks) with the current section; links (internal destinations jump, URIs through task 14's policy); attachment list (save as only); layers (OCG) list with visibility toggles through the virtual file's view state; document properties | 4 | 3, 5 |
| 7 | Text layer and selection | Lazy per-page text with character boxes from the worker, cached and governed; hit testing, drag, double and triple click, Shift+click, select all on page; selection order by logical order with `unicode-bidi` (MIT/Apache-2.0) for RTL lines and PDFium's order for CJK, checked on RTL and vertical CJK fixtures; copy as plain text; the hidden, virtualized accessible text layer for visible pages (§9) | 6 | 4 |
| 8 | Search | Background indexer across the pool (Background priority, batches of 250 pages, document reopened per batch), progress with Cancel, results streamed from the first hit, a capped result list, highlights in the overlay, F3/Shift+F3; case and whole-word options; repeat searches from the text cache; **ADR-0039** (text extraction, cache, index) | 4 | 7 |
| 9 | Annotations | Create, select, move, resize and delete: highlight, underline, strikeout, squiggly, note, free text, ink, square, circle, line and arrow, polygon, standard stamps; author, color, opacity, note contents; appearance streams generated by us for every annotation we create or change (other viewers show them as we do), set with `FPDFAnnot_SetAP`; while dragging, the overlay draws the annotation and the layer isn't re-rendered; every change a transaction with an inverse, coalesced, journaled (§5.7); undo and redo | 9 | 4, 7 |
| 10 | Forms | AcroForm filling through `FORM_*` in the editor worker: text (single, multiline, comb), check box, radio, combo, list box; widgets drawn in the forms layer; text entry through a DOM input over the field so IME works (§9), committed to PDFium; Tab order; field values in the journal; no JavaScript (§3.2): fields whose format, validate or calculate actions won't run get a visible notice; signature fields shown, not signed | 6 | 4, 9 |
| 11 | Page operations and metadata | Reorder (drag in the thumbnail rail), rotate, delete and insert (blank page or pages from another PDF) as updates to the open document; extract, merge and split as new files; outlines and links kept where possible (PDFium `FPDF_ImportPages`, our own outline and destination fix-up); metadata (Info dictionary, and XMP kept in step when present); **ADR-0040** | 7 | 3, 13 |
| 12 | Edits in the core | One edit model for tasks 9–11: transactions of PDF-level operations, the editor worker applies them, render workers reopen the virtual file after a commit; the journal replays them after a crash; external change detection (§5.8) works as for TXT | 3 | 2, 3 |
| 13 | Incremental save | `FPDF_SaveAsCopy(FPDF_INCREMENTAL)` in the editor worker with the copy reads elided; the update checked (opened as original + update in a worker; page count, annotations present) and appended in place per decision B, with the original length in the journal so a crash mid-append is truncated back; "Save optimized copy" as a full rewrite to a new file through the atomic path (§5.8); save reports (§5.8); before the first edit that would invalidate a signature (any edit when a signature's DocMDP forbids it, otherwise per the signature's permissions), a warning; **ADR-0041** | 6 | 12, decision B |
| 14 | Security | No JavaScript ever (§3.2, §6.6); launch actions blocked with a notice; external URIs only after a confirmation that shows the URL; embedded files saved through a dialog, never opened; remote content (§8) none; the worker sandbox from task 2 reviewed; **ADR-0042** | 2 | 6 |
| 15 | Fixtures and corpora | New `tools/fixtures` generators (deterministic): every annotation subtype and an AcroForm; encrypted (RC4 and AES-256, user and owner passwords, through qpdf); a broken xref; RTL (Arabic, Hebrew) and CJK (horizontal and vertical) text with embedded fonts (written with `krilla`, MIT/Apache-2.0, the M6 export library, fixtures only); OCG layers; attachments; links and actions (URI, launch, JavaScript) for the security tests; a 200 × 200 inch page for 6400 %; signed files (pyHanko, MIT, as a test tool in a venv under `.cache/`, with a committed self-signed test key: approval and certification signatures, DocMDP levels). Corpora fetched by script with their licences tracked: the pdf.js test PDFs and PDFium's test files; a corpus run (every page of every file through the workers) nightly in CI | 5 | 1 |
| 16 | Fuzzing | Targets for what we parse from a hostile worker or file: the host's decoder of worker messages (a compromised worker is hostile), the virtual file and save guard, our appearance stream and outline writers' inputs, the text-order code; PDFium itself runs only in workers and is fuzzed upstream (OSS-Fuzz) | 2 | 2, 9, 13 |
| 17 | Benchmarks | PDF rows in `bench/budgets.json`; in-app scenarios `pdf-5000p` (cold first page on both fixtures, page jumps before and after the walk, 800 % zoom, scrolling steady and fling, memory), `pdf-search` and `pdf-save`; `tools/bench/refsim-pdf.ps1` with ADR-0024 stall reporting; criterion benches for the protocol, text ordering and appearance generation | 3 | 5, 8, 13 |
| 18 | WebDriver UI flows | Open from the command line; zoom and fit; thumbnails and outline; go-to-page; search; select and copy; annotate, save, reopen; fill a form; reorder a page; password prompt; repaired banner; worker crash recovery (a worker killed mid-render, the tile comes back) | 3 | 5–13 |
| 19 | DoD checks | Annotations: Edge and Chrome driven by WebDriver (msedgedriver is here; chromedriver fetched) to screenshot our saved files next to our own renders; Acrobat per decision C. Signatures: pyHanko validation and its modification analysis on signed fixtures after our saves; `qpdf --check` on every saved file, untouched pages re-render identically (§10) | 2 | 9, 13, 15 |
| 20 | M3 report | `docs/M3_REPORT.md` with DoD evidence; ADRs; PROGRESS, RISKS, ARCHITECTURE updated | 1 | all |
| | **Total** | | **88** | |

With a 20 % contingency, M3 takes about 106 working days of one engineer's time (M2: 53 planned, 64 with contingency).

## Order

1. Tasks 1–3 first (they need decision A), then 4 and 5: viewing with the first-page and jump rows measured early, since the page walk is the main budget risk.
2. Then 6, 7 and 8; 15 alongside from the start (fixtures and the corpus run guard everything after), 16 as each parser lands.
3. Then 12, 9 and 10; 13 (needs decision B); 11 once saving works.
4. Task 14 alongside 6 and 9 (links and annotations are where actions appear).
5. Tasks 17–19 last, then the report. Run ref-sim with the machine idle and the disk rested (CLAUDE.md).

## Decisions to make in M3 (ADRs)

The ADR index reserved 0011 (PDF engine) and 0012 (structure operations and export); as with 0010 in M2, M3 records them under new numbers and the index rows point there.

- **0035 PDF engine and binaries:** PDFium, prebuilt and pinned, our own FFI; the licences inside the binary (decision A).
- **0036 PDF worker processes:** pool size on hybrid CPUs, protocol and tile memory, crash restart, sandbox, the editor worker.
- **0037 PDF loading:** file access, linearization, the virtual file, the page walk and estimates, files over 4 GiB.
- **0038 PDF tiles and layers:** content, annotation and form layers; zoom buckets; fallback bitmaps.
- **0039 PDF text and search:** extraction, cache, reading order, the indexer.
- **0040 PDF page operations:** in-document updates vs. new files; PDFium import vs. lopdf (MIT) vs. qpdf (Apache-2.0, already a test tool).
- **0041 PDF editing and incremental save:** the edit model, read elision, in-place append (decision B), signatures.
- **0042 PDF security:** actions, URIs, attachments, the worker sandbox.

## Open decisions for the user

**A. PDFium and the licences inside its binary.** `pdfium.dll` from bblanchon/pdfium-binaries (the "prebuilt binaries" §5.1 names) carries, besides PDFium (BSD-3-Clause): abseil, simdutf, llvm-libc (Apache-2.0), fast_float, lcms (MIT), libopenjpeg (BSD-2-Clause), zlib (Zlib), FreeType (FTL, approved 2026-10-08), ICU (Unicode-3.0, approved), and four notice licences not on §0's list: **IJG** (libjpeg-turbo), **libpng**, **libtiff** and the **Anti-Grain Geometry 2.3** notice. All four are permissive (use, modify and redistribute with the notice kept, no copyleft). Options: (1) approve the four, ship a third-party notices file (recommended); (2) build PDFium ourselves without some of them (we'd own a Chromium-style build, gn and depot_tools, on three OSes; JPEG and PNG decoding can't be dropped, so IJG and libpng stay anyway); (3) hayro, which needs none of them but misses the first-page budget and most of M3's scope (spike above).

**B. How an incremental save writes the file.** §5.8 makes saves atomic: write a temporary file, then replace. For PDF that means copying the whole original: on this SSD a 2 GiB write takes 1.1–2.4 s (M1 measurements), so copying the 1 GB fixture costs about 0.5–1.2 s of writing plus reading it, against 667 ms effective; the ≤ 1 s row can't be met above a few hundred MB. Appending the update to the file in place writes only the update (0.7 MB here, milliseconds) and never touches the original bytes, which is also what keeps signatures valid. Its weak point: if the app or the machine dies mid-append, the file has a partial update at its end until quire next starts and truncates it back to the length recorded in the journal; another reader opening it before that would see a damaged last update (most readers repair it from the previous one). Options: (1) append in place for every PDF, with that journal recovery (recommended: one path, meets the budget at any size, original bytes never rewritten); (2) atomic replace for files under 256 MiB and append above (the full guarantee where it's cheap, two paths to test); (3) atomic replace always, and the save row fails for large files (it would need your budget decision).

**C. Checking annotations in Acrobat.** The DoD names Acrobat, Edge and Chrome. Edge and Chrome can be automated here through WebDriver; Acrobat isn't installed, and you declined screen control. Options: (1) you install Acrobat Reader and, once at the end of M3, compare a prepared set of about 12 saved files against our reference renders with a checklist (recommended; about 15 minutes); (2) replace Acrobat with independent renderers I can automate (pdf.js and hayro), a weaker check since Acrobat has its own appearance handling; (3) allow screen control for this one check.

## Risks to watch

- **Cold jumps before the page walk** (spike: 260–920 ms raw against 100 ms effective). The background walk with parallel prefetch has to bring the walk well under the time a user takes to jump; if it can't, I'll bring data and options, never relax the row.
- **PDFium's memory per document** grows with every page it parses (1 GB for a full pass in one document). Batches, recycling and governor accounting are the mitigation; the memory row is measured with workers included.
- **Read elision depends on PDFium's copy pattern.** A PDFium update could change it. Pinned binaries, the guard that serves any unexpected read for real, the check before appending, and a CI comparison of elided and real updates over the corpus.
- **Signature modification analysis.** PDFium's update rewrites the catalog, the page tree and touched page dictionaries (698 KB for one note). Cryptographically the signatures stay valid (the original bytes are untouched), but Acrobat's and pyHanko's modification analysis may flag rewritten objects under DocMDP. Task 19 checks it; if it flags, the update has to be trimmed to the objects that really changed.
- **RISKS #6 (PDFium process model):** form and annotation state lives in one editor worker; render workers see edits through the virtual file. The cost is a reopen per committed edit in each render worker (5 ms open, page lookups warm); measure it against the annotation-drag and typing rows.
- **Hybrid CPUs.** On this machine 4 of 8 cores are low-power: 7 workers did the text pass in 2.1 s against 3.0 s for 3, with per-page cost rising on the E-cores. The pool counts performance cores only, measured on both kinds.
- **Platform frame stalls (RISKS #12)** affect every first-paint, zoom and scroll row; ADR-0024's reporting applies unchanged.
- **Binary supply chain.** The PDFium binary is built by a third party. Pinned by SHA-256, with its `args.gn` (no V8, no XFA) checked; building it ourselves is the fallback if that project stalls, and an M7 hardening option.
