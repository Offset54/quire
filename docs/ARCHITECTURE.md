# Architecture

Status: M0 and M1 implemented. The parts that exist are the scheduler, memory governor, tracing, tile pipeline, virtual scroller, HUD, and the TXT engine and editor (`crates/text`, `src-tauri/src/txt.rs`, `src/txt/`). Measured numbers are at the end.

quire is a Tauri 2 app. A Rust core owns every document: it parses, lays out, rasterizes, searches, edits and saves. The webview is a thin compositor. It draws tiles the core has already rasterized, draws overlays from geometry the core supplies, and forwards input. The frontend never holds a whole document. Native code that can crash or isn't thread-safe (PDFium, the PPT converter) runs in child processes.

## Components

```mermaid
flowchart LR
  subgraph UI["Webview process (TypeScript)"]
    Chrome["App chrome (React)<br/>tabs, rails, inspector, palette"]
    Comp["Compositor (canvas, imperative)<br/>tile layer, overlay layer, virtual scroller"]
    Input["Input surface<br/>keys, IME (EditContext / textarea), pointer"]
    A11y["Hidden accessible text layer"]
  end
  subgraph Core["App process (Rust core)"]
    Cmd["Command and stream layer<br/>invoke, Channels, quire:// scheme"]
    Sched["Job scheduler<br/>Interaction > Visible > NearViewport > Prefetch > Background<br/>cancel token and generation per job"]
    Gov["Memory governor<br/>byte budget, LRU registry"]
    Reg["Provider registry and sniffing"]
    subgraph Prov["Format providers (Document trait)"]
      TXT["text: piece tree, line index, search"]
      MD["markdown: incremental parse, preview blocks"]
      DOCX["docx"]
      PPTX["pptx (also hosts imported PPT)"]
      PDF["pdf proxy"]
    end
    Lay["layout: shaping, UAX #14, paragraph/table/page layout"]
    Ras["raster: display list to tiles (tiny-skia)"]
    OOX["ooxml-common: zip, rels, splice writer, DrawingML"]
    Src["Sources: mmap, or pread + LRU on network/cloud files"]
    Save["Save pipeline, journal, file watcher"]
  end
  subgraph Workers["Child processes"]
    PW["pdfium-worker x N<br/>render, text, annotations, forms"]
    SO["soffice --headless<br/>(optional, user-installed)"]
  end
  Chrome --> Cmd
  Input --> Cmd
  Cmd <--> Comp
  Cmd --> Sched
  Sched --> Prov
  Prov --> Lay --> Ras
  DOCX --> OOX
  PPTX --> OOX
  PDF <-->|"shared memory + pipe"| PW
  PPTX -.->|".ppt import"| SO
  Prov --> Src
  Prov --> Save
  Gov -.-> Ras
  Gov -.-> Prov
```

Every stage caches its output, keyed by (unit, revision, zoom bucket), and every cache registers with the governor. Tiles are 256 or 512 device pixels. On zoom, the compositor scales existing tiles at once and swaps in sharp ones as they arrive. The fallback layer is a low-resolution bitmap per visible unit, so a blank screen is never shown.

## Data flow 1: jump to page 4,000 of a 1 GB PDF

```mermaid
sequenceDiagram
  participant U as User
  participant C as Compositor (TS)
  participant S as Core scheduler
  participant P as PDF provider
  participant W as pdfium-worker k
  U->>C: drag custom scrollbar thumb
  C->>C: map thumb to (page 3999, y offset) using the page-size table<br/>(exact where known, else estimated from known sizes)
  C->>C: draw placeholders now: cached thumbnail if any, else page frame + number
  C->>S: viewport(doc, gen=g+1, pages 3999-4000, zoom bucket, dpr)
  S->>S: bump generation, cancel in-flight jobs from gen g,<br/>drop queued stale jobs, coalesce duplicates
  S->>P: Visible: low-res page bitmaps, then sharp tiles<br/>NearViewport: +-2 pages; Background: text for selection
  P->>W: render(page 3999, tile rect, matrix, cancel id)
  W->>W: FPDF_LoadPage (the worker reads the file through its own<br/>mapping via custom file-access callbacks), FPDF_RenderPageBitmap
  W-->>P: BGRA tile in a shared-memory slot
  P-->>S: tile into the raster cache (counted by the governor)
  S-->>C: tile over the binary transport chosen in ADR-0007
  C->>C: drawImage, replacing the placeholder
```

Budget: a typical page at 100 % in ≤ 150 ms (§4). If a worker crashes, the provider restarts it and re-queues the job, and the user only sees that tile render again.

## Data flow 2: type a character on page 600 of a DOCX

```mermaid
sequenceDiagram
  participant U as User
  participant I as Input surface (TS)
  participant C as Compositor (TS)
  participant D as DOCX provider
  participant L as Layout + raster
  participant J as Journal
  U->>I: keystroke (or IME commit)
  I->>D: apply(Transaction{InsertText(para P, offset, "a")}, base rev)
  D->>D: edit the arena model, record the inverse, rev+1
  D->>J: append the transaction (fsync batched, at most 1/s)
  D->>L: relayout P only (shaping cache per run, line break at P's width)
  alt P keeps its height
    L->>L: patch page 600's display list, re-raster dirty tiles only
  else P's height changes
    L->>L: re-lay out page 600 now at Interaction priority
    L-->>D: Background job: re-paginate forward from page 601,<br/>stop once page boundaries match the old layout again
  end
  D-->>C: caret and selection geometry, dirty tiles (same rev)
  C->>C: draw caret overlay and swap tiles in the same frame
```

Budget: input to visual response p95 ≤ 16 ms (§4). While pagination converges, the status bar shows an estimated page count ("~1,240 pages").

## Data flow 3: type a character in a 2 GB TXT file

TXT is the exception to core-rendered tiles that §5.4 allows: the core lays out rows and the webview draws them as DOM text (ADR-0023).

```mermaid
sequenceDiagram
  participant U as User
  participant V as TxtView (TS)
  participant H as Host (txt.rs)
  participant T as quire-text
  participant J as Journal thread
  U->>V: keystroke (beforeinput) or IME commit
  V->>V: local echo when certain to match: insert into the caret row,<br/>shift later rows and the caret, draw in the next frame
  V->>H: txt_edit(action, view request around the caret)
  H->>T: apply: append to the add buffer, split/merge pieces (O(log n)),<br/>record the inverse; CRLF junctions rejoined
  H->>J: records (written at once, fsync at most 1/s)
  H->>T: view::build: rows around the caret, one binary message
  T-->>V: header (selections, counts, host time) + rows
  V->>V: adopt the rows (re-echo keystrokes still queued),<br/>redraw rows whose text or decorations changed
```

The original stays mapped (ADR-0007); a 64 KiB-chunk line index fills in the background, and line lookups before it reaches a region are estimates labelled as such. Search, save and replace-all read immutable snapshots (one `Arc` clone of the persistent piece tree, ADR-0008) on the scheduler while editing continues. Non-UTF-8 files are decoded once in parallel segments; untouched ranges are copied raw on save (ADR-0009).

## Invariants

- No task on the UI thread runs longer than 8 ms. Bulk data never crosses IPC as JSON or base64.
- Saving writes a temp file, fsyncs it, then atomically replaces the original. The mapped-file sequence is in §5.8. Every save produces a save report.
- Content the app doesn't understand is kept byte-for-byte. Spliced XML ranges and raw-copied zip entries are the default write path.
- Every parser returns errors instead of panicking. Panics are caught at the provider boundary, and native code runs in worker processes.

## Measured in M0

Dev machine, AC power. "ref-sim" means the ADR-0021 profile: pinned to the 4 P-cores, 1920×1080 device px. Raw JSON is in `bench/results/m0/`.

| Path | Number | Source |
|---|---|---|
| Scheduler hand-off (Interaction job) | 5.7 µs | criterion, ADR-0003 |
| Rasterize one 512 px tile (rects + polygons) | 0.77 ms | `refsim calibrate`, ADR-0006 |
| Core → canvas, one raw 512 px tile, WebView2 SharedBuffer | 2.7 ms p50 / 4.2 ms p95 | Spike, ADR-0004 |
| Same over `invoke` (fallback), QOI text tile | 3.6 ms p50 | Spike, ADR-0004 |
| Full 1920×1080 repaint, SharedBuffer raw | 26 ms | Spike, ADR-0004 |
| 10M-row list, steady scroll / fling, ref-sim | p99 frame 8.6 / 8.7 ms, 0 placeholders, compose work 0.9 ms p95 | ADR-0005 |
| Jump to a random row, until sharp, ref-sim | 56 ms p50, 407 ms max | `refsim-2026-10-08-*.json` |
| App start, process creation → first frame, ref-sim warm | 480 ms p50 (bare WebView2 floor: 449 ms) | ADR-0022 |

The bottleneck is at startup, in WebView2's own process launch, not in this pipeline. Once the app is running, tile delivery and composition stay well inside the frame budget.

## Measured in M1

Ref-sim profile unless noted; raw JSON in `bench/results/m1/`. Run 1 is `refsim-txt-2026-10-09-102220.json` (ffa3e55), run 2 `refsim-txt-2026-10-09-104421.json` (51e0a14).

| Path | Number | Source |
|---|---|---|
| `view::build`, 175 rows of the 2 GiB fixture, core only (raw) | 1.1 ms p50 unwrapped, 0.4 ms wrapped | `viewbench` example |
| Edit round trip: host time / IPC | ≈ 2 ms / ≈ 5.5 ms | `typing.coreMs`, `roundTripMs` |
| Typing → visual response (local echo), ref-sim | p95 8.1 / 9.4 ms, p99 8.4 / 10.1 ms (runs 1 / 2) | ADR-0023 |
| 2 GiB first paint, cold | 149 ms (run 1); run 2 hit platform frame stalls | RISKS #12 |
| Line index, 2 GiB cold | 0.58–0.76 s (all six runs) | |
| Literal search, 2 GiB cold, full pass | 1.14 s / 0.38 s (medians, runs 1 / 2) | |
| Save after a small edit, 2 GiB, core only (raw) | 0.90–1.05 × a raw sequential write | `savebench` example |
| 300 MiB single line, first paint (unpinned) | ≈ 146 ms | after the parallel long-line scan |