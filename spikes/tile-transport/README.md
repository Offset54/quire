# Tile transport spike (M0 task 7, ADR-0004)

Measures how fast a rendered tile gets from the Rust core onto the webview's
canvas. Not part of the product; nothing here ships.

- Transports: async custom URI scheme, `invoke` returning a binary `Response`,
  a per-request `Channel`, and WebView2 `PostSharedBufferToScript` (Windows).
- Encodings: raw RGBA, LZ4 block, QOI. The JS decoders are verified
  byte-for-byte against the raw tiles before timing starts.
- Content: "text" (white page with glyph-like marks) and "photo" (noise).
- Tile sizes 256 and 512 device px. The window is 1280×720 logical, which is
  1920×1080 device px at 150 %, the reference machine's viewport.
- Scenarios: single-tile latency (80 sequential tiles after 10 warm-up) and
  full-viewport repaint (every tile requested at once, 15 repetitions after
  2 warm-up), with frame-gap and long-task monitoring.

Run (on AC power):

```powershell
cd spikes/tile-transport; cargo tauri build --no-bundle; cd ../..
$env:SPIKE_OUT = "$PWD\.cache\spikes\tile-transport.json"; .\target\release\spike-tile-transport.exe
```

Progress goes to `$SPIKE_OUT.log`; a hung transport times out after 5 s and is
recorded under `failures`. Committed results: `bench/results/spikes/tile-transport/`.
