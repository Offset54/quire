# 0004. Tile transport from core to canvas

- Status: Accepted for Windows. macOS and Linux are provisional until measured there (see Consequences).
- Date: 2026-10-08

## Context

§5.4 and §5.6 make the Rust core render tiles and the webview only composite them. The transport between the two sets the floor for every visual budget: typing p95 ≤ 16 ms, 60 fps scrolling, PDF page ≤ 150 ms, and DOCX first page ≤ 1 s. JSON and base64 are ruled out. The candidates were Tauri's three binary paths and WebView2's shared buffers, each combined with raw RGBA, LZ4 or QOI.

## Method

The spike app is in `spikes/tile-transport/`; its README has the details. Measurements were taken on 2026-10-08 on the dev machine (Core Ultra 7 258V, AC power, Balanced power plan, WebView2 154). The viewport was 1920×1080 device px at DPR 1.5, matching the reference machine's display exactly. The CPU was not pinned or derated, because the ref-sim harness comes in task 12; derated estimates are given below.

There were two full runs, and they agreed within about 10 %. Raw results are in `bench/results/spikes/tile-transport/2026-10-08-{a,b}.json`. Every timing ends at a drawable `ImageBitmap`: request → bytes in JS → decode → `createImageBitmap`.

Compression on the synthetic tiles:

| Content | LZ4 | QOI |
|---|---|---|
| Text page | 6.6 % of raw | 5.3 % of raw |
| Photo / noise | 100.4 % | 94.1 % |

## Results (run a, p50)

**Single tile, request to bitmap (ms, p50 / p95):**

| Transport | raw 256 | raw 512 | QOI text 256 | QOI text 512 |
|---|---|---|---|---|
| WebView2 SharedBuffer | **1.5 / 2.6** | **2.7 / 4.2** | 1.4 / 2.7 | 2.9 / 5.5 |
| `invoke` Response | 3.8 / 4.4 | 12.5 / 13.4 | 1.9 / 2.9 | 3.6 / 5.2 |
| Custom URI scheme | 4.1 / 6.1 | 12.4 / 12.9 | 1.9 / 2.5 | 3.8 / 5.2 |
| Channel (one per request) | 5.2 / 7.1 | 13.7 / 19.0 | 2.9 / 4.6 | 5.2 / 7.6 |

**Full 1920×1080 repaint, all tiles at once (ms, p50):**

| Transport | raw 256 (40 tiles) | raw 512 (12 tiles) | QOI text 512 | QOI photo 512 |
|---|---|---|---|---|
| SharedBuffer | 45 | **26** (≈ 490 MB/s) | 36 | 58 |
| `invoke` | 71 | 83 (≈ 150 MB/s) | 34 | 89 |
| URI scheme | 69 | 84 | 35 | 85 |
| Channel | 89 | 94 | 44 | 96 |

Observations:

1. Tauri's standard binary paths all plateau around **150 MB/s**. A raw 512 px tile (1 MiB) costs about 12 ms in transit, which alone uses most of a frame.
2. **SharedBuffer moves raw tiles about 3× faster** and needs no decoding. It is the only option that keeps a 512 px tile under 3 ms without depending on how compressible the content is.
3. Compression helps the standard paths only when the content compresses: QOI text tiles are about 4× faster over `invoke`, but photo tiles are no faster.
4. **Decoding on the UI thread causes jank.** During repaints that decode LZ4/QOI in JS, 2–32 frames went over 20 ms. Raw SharedBuffer at 512 px had 0–1 such frames. Even raw transfers at 256 px had 11–14 slow frames, from the overhead of handling 40 tiles at once. No long tasks over 50 ms were observed.
5. **Channel gotcha.** Tauri numbers Channel messages per command invocation. Reusing one JS Channel across invocations makes every later message look out of order, and they are buffered forever. This hung the first run.

## Decision

1. **Windows: WebView2 SharedBuffer, raw RGBA.** Buffers are posted from the UI thread through `with_webview`, and the script releases each one once its `ImageBitmap` exists.
2. **Default tile size is 512 device px** for viewing: it has the best repaint throughput and the fewest per-tile costs. For editing, the dirty-region path sends **sub-tile patches** (only the changed rectangle) rather than whole tiles; that design is owned by M4. The tile size stays a parameter.
3. **Fallback** where SharedBuffer is unavailable (macOS, Linux, or a WebView2 failure at runtime): `invoke` with a binary Response. Each tile is QOI-encoded and sent raw instead if QOI saves less than 20 %. **Decoding and `createImageBitmap` run in a Web Worker**, so they never block the UI thread; task 9 builds this.
4. The Channel transport isn't used for tiles. If channels are used for anything else, the rule is one Channel per logical stream, created alongside its command.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| JSON / base64 | — | Ruled out by §5.6 |
| Async custom URI scheme | Tauri (MIT/Apache-2.0) | Same ~150 MB/s plateau as `invoke`. It also needs the CSP to allow a second origin. |
| Tauri Channel | Tauri (MIT/Apache-2.0) | Slowest in every case, plus the ordering gotcha above |
| LZ4 (`lz4_flex`, MIT) | MIT | QOI compresses document tiles slightly better and decodes about as fast |
| QOI (`qoi`, MIT/Apache-2.0) | MIT/Apache-2.0 | Used in the fallback only. On SharedBuffer, decoding costs more than it saves. |
| localhost WebSocket | — | Adds a network listener (attack surface, §8) and wasn't needed |

## Consequences

- With the ref-sim derating (D = 1.5, ADR-0021), a single 512 px tile on Windows is an estimated 4 ms p50 / 6 ms p95. That leaves room inside the derated 10.7 ms typing budget for layout and rasterizing. A full repaint is an estimated 39 ms (≈ 2.4 frames), so jumps and zooms show scaled or placeholder tiles for a couple of frames, which §5.4 allows. Task 12 will measure these numbers in ref-sim rather than estimate them.
- The SharedBuffer path is Windows-only `unsafe` COM code. It sits behind a `TileTransport` trait with the `invoke` fallback, and both paths are tested.
- **macOS and Linux numbers are unknown.** Their fallback path has to be measured on real hardware before M7, and this ADR updated (risk #5). CI cannot measure it meaningfully.
- Open follow-ups:
  - Buffer pooling. Each post currently creates a new shared buffer; pooling could cut the remaining ~1 ms.
  - Frame pacing of large bursts: spread decode and `createImageBitmap` across frames.
