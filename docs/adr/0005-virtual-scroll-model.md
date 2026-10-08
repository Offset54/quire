# 0005. Virtual scroll model and tile pipeline

- Status: Accepted
- Date: 2026-10-08

## Context

§5.4 requires that 50M-line text files and 10,000-page PDFs scroll and jump correctly. Browsers cap element heights at a few tens of millions of pixels: 10M rows × 30 device px is already 3·10^8 px, so native scrolling can't represent these documents. §4 requires 60 fps scrolling with no frame over 50 ms; placeholders are allowed, a blank screen isn't. The M0 DoD is "a synthetic 10M-row list scrolls at 60 fps".

## Decision

**Scroll model.** Nothing scrolls natively.
- A surface's position is one number: the offset in device pixels from the top of the document. JS numbers and Rust `f64` are exact up to 2^53 px.
- Tiles are addressed by `(view, layout, col, row: u64, size)`. Only the small tile-relative coordinates are narrowed to `f32` for rasterizing.
- The canvases stay fixed to the viewport. Wheel, keyboard and a custom scrollbar change the offset.
- The scrollbar thumb maps proportionally to the offset, with a 24 CSS px minimum length (`ScrollModel`).

**Surface layers** (`DocumentSurface`, imperative TypeScript outside React):
- A tile canvas draws cached `ImageBitmap`s at `row·size − offset`, and a placeholder wherever a tile is missing.
- An overlay canvas draws caret, selection and similar from a painter callback, without touching the tiles.
- The tile cache is LRU, sized at ten viewports of tiles.

**Requests.** The frontend computes the tiles it needs and asks the core for the ones it doesn't have (`view_request_tiles`), at most once per frame while the view moves, and every 250 ms while any visible tile is still missing.
- Priorities: visible, then near (half a viewport each side), then prefetch (one more viewport in the scroll direction).
- When the view moves more than a viewport between requests (thumb drags), only visible tiles are requested.
- Each request bumps the view's `Generation`, so queued tiles that are no longer wanted are dropped before rendering.

**Duplicate suppression.** The core ignores re-requests for a tile it delivered in the last 400 ms, because the tile is probably still on its way. The frontend drops arriving tiles outside `wantedRange` (the union of every range it could request) before decoding them. Both windows come from one function, and a unit test checks that a requested tile is never dropped on arrival.

**Delivery** follows ADR-0004.

## Evidence

The `scroll-list` scenario, run twice:
- Machine: raw dev machine, AC power, 144 Hz panel (refresh interval 6.9 ms).
- Viewport: 1902×1080 device px, which is 1920 minus the 12 CSS px scrollbar.
- Document: 10M rows of 30 device px, 512 px tiles.
- Results are in `bench/results/m0/scroll-list-2026-10-08-*.json`. These are not ref-sim numbers; the ref-sim profile comes with task 12.

| Phase | Transport | Frame interval p95 / max | Dropped frames | Frames with placeholders | Compose work p95 |
|---|---|---|---|---|---|
| Steady 2,400 CSS px/s, 6 s | SharedBuffer | 7.1 / 7.3 ms | 0 | 0 % | 1.1 ms |
| Fling 24,000 CSS px/s, 4 s | SharedBuffer | 7.1 / 7.3 ms | 0 | 0 % | 0.8 ms |
| Thumb drag over all 10M rows, 3 s | SharedBuffer | 7.0 / 7.4 ms | 0 | 100 % (expected) | 0.9 ms |
| Steady | Channel + QOI worker | 7.1 / 7.5 ms | 0 | 0 % | 1.1 ms |
| Fling | Channel + QOI worker | 7.1 / 7.2 ms | 0 | 0.7 % | 1.0 ms |
| Thumb drag | Channel + QOI worker | 14.1 / 27.8 ms | 153 at 144 Hz | 100 % | 0.8 ms |

Other measurements:
- First paint after opening: 38 ms.
- Jump to a random row until every visible tile is sharp: 43 ms p50, 311 ms max (SharedBuffer); 61 ms p50 (Channel).
- No long tasks in any phase.
- Snapshots after first paint and at the very end showed rows 0–35 and rows 9,999,964–9,999,999, correctly drawn with no visible seams.

During the thumb drag, each frame lands about 100,000 rows further on, so no tile can arrive before it's obsolete. Frames stay smooth and show placeholders until the drag pauses, which §4 allows. Even the slower fallback path stays above 60 fps.

Problems found and fixed while measuring:
1. **Re-rendering tiles still in flight.** Steady scrolling received 4,125 tiles in 6 s instead of about 180, flooding the UI thread. Fixed with duplicate suppression.
2. **Prefetched tiles dropped on arrival.** The drop window was narrower than the prefetch window, which caused placeholders on 98 % of fling frames. Fixed by deriving both from `wantedRange`.

## Alternatives considered

| Option | Why not |
|---|---|
| Native scrolling with a tall spacer element | Breaks above ~33.5M px in Chromium and isn't exact well before that |
| Scaling the offset (`scrollTop × k`) | Loses row precision in huge documents; wheel deltas become uneven |
| Core-driven push of whatever the core thinks is visible | The frontend owns the exact viewport and cache. Asking only for what's missing keeps the core stateless about the frontend's cache. |

## Consequences

- Every surface (TXT, MD preview, PDF, DOCX, PPTX) uses this model.
- Variable-height units (pages, lines) need a provider-side offset↔unit index (the line index in M1, page sizes in M3). The extent can be an estimate (`exact: false`) that refines over time; the viewport is anchored to the unit under its top edge, so it doesn't jump when the estimate changes (§6.1).
- Frame metrics here are on a 144 Hz panel and a fast machine. The ref-sim run (task 12) re-checks them with derated thresholds.
