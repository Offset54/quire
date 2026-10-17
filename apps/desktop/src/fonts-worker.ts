// Font warm-up off the UI thread (src/fonts.ts): measuring the samples on an
// OffscreenCanvas runs the platform's font fallback and loads the faces in
// this worker; the process-wide typeface cache then answers the page's own
// layouts quickly.

import { FONT_SAMPLES } from "./font-samples";

const ctx = new OffscreenCanvas(1, 1).getContext("2d");
const times: number[] = [];
if (ctx) {
  for (const [font, text] of FONT_SAMPLES) {
    const t0 = performance.now();
    ctx.font = font;
    ctx.measureText(text);
    times.push(performance.now() - t0);
  }
}
postMessage(times);
