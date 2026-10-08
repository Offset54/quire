# 0006. Rasterizer: tiny-skia baseline

- Status: Accepted for M0–M3; to be revisited in M4 (vello_cpu) with text-heavy tiles
- Date: 2026-10-08

## Decision

tiny-skia 0.12 (BSD-3-Clause), CPU, wrapped by `quire-raster::TileCanvas`:
- callers draw in f64 document coordinates;
- tiles are RGBA8 and opaque, so premultiplied and straight values are identical.

## Alternatives considered

| Option | License | Why not (for now) |
|---|---|---|
| vello_cpu | Apache-2.0/MIT | Promising for path-heavy pages. It will be benchmarked in M4 on real DOCX/PPTX display lists, where text and paths dominate. |
| GPU vello | Apache-2.0/MIT | Driver variance across integrated GPUs, and readback to the transport adds latency |
| skia-safe | MIT + Skia BSD-3 | Heavy C++ build and binary size |
| Direct2D | Windows only | Not portable |

## Evidence

- `refsim calibrate`: 64 tiles of 512 px with 200 rectangles and 40 polygons each in 49 ms, or **0.77 ms per tile**.
- The synthetic list's tiles render in about 0.6 ms each (HUD job history).
- Both are far below the tile transport cost (2.7 ms, ADR-0004), so rasterizing isn't the bottleneck in M0.
