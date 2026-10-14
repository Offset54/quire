# 0030. Markdown preview surface: byte-anchored DOM blocks in windows the host sends

- Status: Accepted
- Date: 2026-10-14

## Context

§6.2: a virtualized preview with height estimates and a measurement cache; a source map (block ↔ byte range) drives two-way scroll sync and click-to-source; KaTeX and Mermaid render lazily. §4 (MD 20 MB): first paint of source and preview ≤ 500 ms (cold file), keystroke → preview update ≤ 50 ms; the cross-cutting rows: 60 fps scrolling with no frame over 50 ms, typing p95 ≤ 16 ms / p99 ≤ 33 ms, no UI-thread task over 50 ms. §5.5: never one whole-file buffer, webview caches count against the memory governor. User decision C: Obsidian-like panes (editing view, reading view on Ctrl+E, split view on demand).

The core already renders each top-level block to sanitized HTML (ADR-0026 to ADR-0028). The question is how that reaches the screen: as DOM in the webview, or as tiles the core rasterizes (the path PDF and Office take, ADR-0004/0005).

## Decision

**DOM blocks, not core tiles.** The preview shows rendered blocks as DOM elements in the webview (`apps/desktop/src/md/preview.ts`). Rich text layout, selection and copy, links, accessibility, images, and later KaTeX and Mermaid output (HTML and SVG) are what the webview does well; rasterizing them in the core would mean writing a second HTML layout engine. Tiles stay the path for fixed-layout formats (M3+). The costs (layout on the UI thread, a DOM that must stay small) are handled below.

**Byte anchor, no document height.** The position is (block, pixels into it); the scrollbar is a fraction of bytes (ADR-0005's model). Only the blocks of the current window are in the DOM, in normal flow inside a layer translated so the anchor sits at the viewport's top. Heights are measured from the DOM each frame, so a block that changes height above the anchor (an image loading, a late rendering) never moves what is on screen; nothing estimates the whole document's height. Height estimates (line count × 22 px, or the last measurement of the same rendering) only decide how many blocks to place.

**Windows from the host.** The frontend asks for a window around a byte offset (`md_view`: 40 blocks before, 80 after) and gets a binary `MDW1` message: per entry its id, byte range, kind, line count and rendering revision, and HTML only for renderings the frontend doesn't have (the host remembers what it sent; the frontend lists what it dropped). The frontend keeps rendered HTML by id (up to 3,000 entries) and reconciles windows by id. It fetches again when the anchor nears a window end. After an edit, or when the background index completes, the host's per-document worker pushes the window again on the document's Channel. Window ends are explicit flags.

**Rendering cache with dependency signatures** (`crates/markdown/src/preview.rs`): a rendering is reused while the table values it used (definitions it references, footnote numbers, heading ids) hash the same. After an edit the worker pushes a *provisional* window first, rendered against the tables as they are, then *corrections* once the tables are rebuilt, for blocks whose values changed (a renumbered footnote, a link whose definition was edited far away).

**Progressive placement.** A frame places the blocks that cover the viewport plus half a viewport above and below; the rest of the window is placed over the following frames within 6 ms per frame. Placing a whole 120-block window in one frame took 62–64 ms (md-20m, raw), over §4's 50 ms.

**Large blocks as pieces** (`crates/markdown/src/pieces.rs`). Tables over 100 rows and fenced code blocks over 200 lines are shown as pieces of 16–128 rows (lines), each an entry of its own with its own source range. Cuts fall after rows whose hash hits a residue, so an edit changes only its piece (inserting a row moves no other boundary). Each piece renders the block's head (a table's header and delimiter rows, the opening fence), its rows and the tail (a closing fence). Table pieces are separate `<table>` elements with the same column widths (a `<colgroup>` from the widest cell per column over the whole table, `table-layout: fixed`; `width` attributes, since the CSP allows no inline styles); only the first shows the header. A window holds up to 8 pieces each side of the anchor's and ends at a pieced block that doesn't hold the anchor. Before pieces, a keystroke inside the fixture's 1,500-row table took 77–115 ms to reach the screen (raw), with a long task each time.

**Hidden panes draw nothing.** In the editing view the preview is hidden; its blocks would measure 0 px and the anchor would run to the window's end. It draws again when shown (the resize observer).

**Font warm-up.** The first layout that uses a font face loads it, and text in a script the stack's first font lacks runs Windows font fallback, 10–30 ms per face on the UI thread. With Latin, Hangul and emoji on the first screen that came to 50–72 ms in one task when opening the fixture. After the app's first frame, samples of those scripts are laid out in the editor and preview stacks, one per task (`apps/desktop/src/fonts.ts`). Other scripts still pay on first use (RISKS #13).

**Scroll sync** follows the pane the user touched last (wheel, keys, pointer); the other pane follows by byte offset, interpolated within the block, and never drives back.

## Measurements

Ref-sim (ADR-0021: performance cores, 8 logical CPUs, 1280×720 logical), 20 MiB fixture, `tools/bench/refsim-md.ps1`, `bench/results/m2/`:

- First paint of both panes, cold file, split view: 86–93 ms (budget 500 ms, 333 ms effective); no long task while opening.
- Preview scrolling: steady max frame 9.1–9.7 ms, fling 9.1–16 ms (budget 50 ms; the panel runs at 144 Hz).
- Keystroke → preview update (100 keystrokes per run at five positions, 120 ms apart): p50 12–15 ms per position. 14 of 100 keystrokes per run, those after the pause, hit a platform frame stall (RISKS #12): 150–370 ms raw. The other 86: p95 22–24 ms, max 29–31 ms (33.3 ms effective). Stalled samples also receive their window later (median 13–15 ms against 7–8 ms), so subtracting the frame wait alone (ADR-0024's rule for jumps) leaves p95 38.8 ms and max 47.4 ms. Judged on the keystrokes the platform did not stall (user decision 2026-10-15, ADR-0024 addendum); stalled ones are reported with each run.
- Corrections after an edit (renumbered footnotes, links to an edited definition): p50 28–34 ms.
- Typing in the source view: p95 8.6 ms; p99 8.4–18.3 ms, and 187 ms in one run where a stall hit (judged raw, ADR-0024).
- Memory after the index completes: +55 MiB private bytes.

## Consequences

- Layout cost is ours to budget: every new kind of rendering (KaTeX, Mermaid, highlighting) must keep a placed block cheap, or be split, or arrive later as a labeled placeholder (§7).
- Blocks other than tables and fenced code (a 5,000-item list, a huge HTML block) are still one entry each; up to 512 KiB they render whole, beyond it only their start, labeled. A list of that size would cost a long task to place. Watched in RISKS.
- A piece renders its rows without the rows before it. Footnote numbers are document-wide and stay right, but the back-reference ids of references to one note from two pieces of the same table are the same (`fnref-1-1` twice), so the note's back-links lead to the first of them. Recorded in RISKS.
- The frontend keeps HTML for up to 3,000 entries and reports its size to the memory governor (`md-preview-dom`), like decoded preview images (`preview-images`).
- `data-sourcepos` in piece HTML counts lines from the piece's own source; the frontend uses the entries' byte ranges instead.
