# 0023. TXT surface: virtualized DOM rows from core layout, with local echo for typing

- Status: Accepted
- Date: 2026-10-09

## Context

§5.4 makes the core render DOCX, PPTX and PDF text into tiles so measurement and painting agree, but allows TXT and MD source views to "use virtualized DOM text for the visible lines, which helps IME and accessibility". Risk #4 (IME on canvas surfaces) is high likelihood and high impact. The TXT budgets (§4) that the surface has to carry: typing p95 ≤ 16 ms and p99 ≤ 33 ms, 60 fps scrolling with no frame over 50 ms, first paint of a 2 GB file ≤ 300 ms, and no hang on a 300 MB single line (caret moves, wrap toggle).

The choice was between:

1. **Core tiles**, the same path as the paginated formats: the core shapes and rasterizes text into 512 px tiles sent over SharedBuffer (ADR-0004), and the frontend composites them with an overlay for caret and selection.
2. **DOM rows**: the core does the layout that depends on the whole file (line bounds, wrap rows, column windows for long lines) and sends the visible rows as text, and the frontend renders each row as a DOM text node.

The task 10 spike was not run as a separate prototype. The decision rests on the M0 tile-transport measurements and on end-to-end measurements of the DOM implementation in M1, below.

## Decision

The TXT view (`apps/desktop/src/txt/txt-view.ts`) renders **virtualized DOM rows**:

- **The core owns layout.** `quire-text::view::build` returns one binary message (§5.6; no JSON for text) with the rows around an anchor: offsets, flags, the line number, the visual column of the first character, the indices of characters that stand for one raw byte (invalid UTF-8 shown as U+FFFD, controls as U+2400 symbols), and the text. Wrapping (`layout`) is computed in the core per 64 KiB segment, so a 300 MB line costs at most one segment of work. Rows longer than 8 KiB carry only the text around the visible columns.
- **The frontend owns drawing.** A pool of row elements is positioned with transforms inside a layer that moves by fractions of a row; the scroll model is the byte-anchored virtual scroller of ADR-0005 with custom scrollbars. Row keys depend on the row's text and line number only, so an edit redraws the rows that changed, not every row after it.
- **Overlays** (caret, selections, search highlights, IME composition) are per-row divs positioned from DOM Range measurement of the row's text, so they always line up with what the browser drew.
- **Input** goes through a hidden textarea placed at the caret (the IME candidate window follows it); composition text is shown inline in the caret's row and committed once.
- **Local echo.** A keystroke is drawn in the next frame and confirmed when the core answers, because a round trip per keystroke (host ≈ 2 ms, IPC ≈ 5.5 ms on this machine) plus the wait for the next frame does not fit the derated budget. The echo is used only when its result is certain to match the core's: one empty selection, an ordinary row (not windowed), no other edit or fetch pending, and with wrapping on, no row that could reflow. Keystrokes typed while a request is in flight are echoed again on top of each response; an error drops the echoes and refetches. The bench compares every echoed row with the core's result.

## Evidence

Ref-sim profile (ADR-0021: 4 P-cores, 1280×720 logical at 150 %, derated thresholds), this machine (Core Ultra 7 258V, Arc 140V, 144 Hz panel), `bench/results/m1/refsim-txt-*.json`, median of three cold runs:

| Metric | Run 1 (ffa3e55) | Run 2 (51e0a14) | Effective budget |
|---|---|---|---|
| Typing → visual response p95 / p99 | 8.1 / 8.4 ms | 9.4 / 10.1 ms | 10.7 / 22 ms |
| Typing: keystrokes echoed, mismatches | 300 / 0 | 300 / 0 | |
| Steady scroll p99 / max frame | 7.3 / 13.5 ms | 8.6 / 244 ms ¹ | 16.7 / 50 ms |
| Fling p99 / max frame | 7.2 / 8.2 ms | 8.5 / 8.6 ms | 16.7 / 50 ms |
| Long tasks during scrolling and typing | 0 | 0 | 0 |
| 300 MB line: caret moves max, wrap toggle max | 17.3, 27.9 ms | 34.4, 30.4 ms | 66.7 ms |
| 300 MB line wrapped, scroll max frame | 7.3 ms | 341 ms ¹ | 50 ms |

¹ Frames the platform did not present: the GPU process was blocked in DXGI Present while the renderer was idle (long-animation-frame blocking duration 0). Plain Microsoft Edge on this machine shows the same stalls (`bench/results/m1/edge-idle-probe-2026-10-09.json`); see RISKS #12. They are not caused by the DOM-row surface and would affect a tile surface the same way.

Before the M1 fixes, typing was 16 ms p50 / 21 ms p95 unpinned: the round trip, then a redraw of every visible row (the old row keys included the revision), then the frame wait. With the core's view build at 1.1 ms p50 (it was 50 ms before `chars()` was made linear), the frontend cost is a few rows of DOM text per keystroke.

The tile alternative, from ADR-0004 (raw dev machine): a 512 px tile reaches the screen in 2.7 ms p50 over SharedBuffer and a full 1080p repaint takes 26 ms. Typing on tiles would add shaping and glyph rasterization in the core (a text stack the plan schedules for M4) to every keystroke, and IME and screen readers would need a parallel hidden text model anyway.

UI flows through WebDriver (`tests/ui/txt-flows.test.mjs`, 8 of 8 passing): typing, undo and redo; Korean IME composition committed exactly once; a burst of typing faster than the core answers keeps every character in order and backspaces queued behind it remove them; go to line; find and replace-all; save; kill and restore.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| Core-rendered tiles for TXT | — | Needs core shaping and glyph raster per keystroke (M4 work), a parallel accessibility tree and an IME proxy. No budget gain: the DOM path already meets typing and scrolling. |
| Canvas `fillText` in the frontend | — | No IME or accessibility from the platform; text measurement in JS per frame. |
| `contenteditable` over the visible rows | — | The browser's editing model would fight the core's (multi-cursor, column selection, byte-exact edits); selection and undo across the virtual window can't be made consistent. |
| EditContext API | — | WebView2 only (not WKWebView or WebKitGTK, risk #4); can be added behind the textarea later without changing this design. |
| No local echo | — | Typing p95 21 ms unpinned against 10.7 ms effective: a round trip plus a frame wait doesn't fit. |

## Consequences

- The core's wrap columns use Unicode East Asian Width; the browser draws with real font metrics. With a monospace font they agree for Latin, CJK and Hangul; emoji and fallback fonts can be wider or narrower than two columns, so a wrapped row may end slightly short of or past the view edge. Positions, selections and the caret are always measured from the DOM, so editing is unaffected.
- Only visible rows (plus overscan) exist in the DOM. Screen readers read the visible window; a full accessibility review is planned for M7.
- The local echo duplicates a small part of the editing model in the frontend (insert at one caret). Its conditions are deliberately narrow, the bench reports every mismatch, and anything outside them waits for the core.
- MD source view (M2) reuses this surface. DOCX/PPTX/PDF stay on core tiles (§5.4).
- Not yet verified: the Microsoft IMEs (Korean, Japanese, Chinese) by hand. The UI flow sends the composition events the Korean IME sends; manual QA with the real IMEs is listed in the M1 report. Risk #4 stays open for the canvas surfaces of M4.
