// M0 DoD scenario: "a synthetic 10M-row list scrolls at 60 fps" (§11).
//
// Phases, each measured separately:
// - firstPaint: open to every visible tile drawn sharp
// - steady: constant 2,400 CSS px/s for 6 s (fast wheel scrolling)
// - fling: constant 24,000 CSS px/s for 4 s
// - drag: scrollbar thumb dragged top to bottom over 3 s (whole document)
// - jumps: 15 random jumps; time until every visible tile is sharp

import { invoke } from "@tauri-apps/api/core";
import { getLongTaskCount } from "../perf/trace";
import { DocumentSurface, type FrameSample, type ViewInfo } from "../surface/surface";
import { summarize } from "../perf/stats";

const ROWS = 10_000_000;
const ROW_CSS_PX = 20;

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function measureRefresh(): Promise<number> {
  let last = await nextFrame();
  const gaps: number[] = [];
  for (let i = 0; i < 90; i++) {
    const t = await nextFrame();
    gaps.push(t - last);
    last = t;
  }
  return summarize(gaps).p50;
}

async function untilSharp(surface: DocumentSurface, timeoutMs = 10_000): Promise<number> {
  const start = performance.now();
  while (surface.missingVisible() > 0) {
    if (performance.now() - start > timeoutMs) return NaN;
    await nextFrame();
  }
  return performance.now() - start;
}

/** Runs `step(dtMs, elapsedMs)` every frame for `durationMs`, recording frames. */
async function phase(
  surface: DocumentSurface,
  refresh: number,
  durationMs: number,
  step: (dt: number, elapsed: number) => void,
) {
  const samples: FrameSample[] = [];
  const stop = surface.onFrame((s) => samples.push(s));
  const before = { ...surface.counters };
  const gaps: number[] = [];
  const longBefore = getLongTaskCount();
  const start = await nextFrame();
  let last = start;
  while (last - start < durationMs) {
    const t = await nextFrame();
    gaps.push(t - last);
    step(t - last, t - start);
    last = t;
  }
  stop();
  const missingFrames = samples.filter((s) => s.missingTiles > 0).length;
  const missingShare = samples.map((s) => s.missingTiles / Math.max(1, s.visibleTiles));
  return {
    frameIntervalMs: summarize(gaps),
    droppedFrames: gaps.filter((g) => g > refresh * 1.5).length,
    framesOver50ms: gaps.filter((g) => g > 50).length,
    composeWorkMs: summarize(samples.map((s) => s.workMs)),
    framesWithPlaceholdersPct:
      Math.round((missingFrames / Math.max(1, samples.length)) * 1000) / 10,
    meanPlaceholderSharePct: Math.round(summarize(missingShare).mean * 1000) / 10,
    longTasks: getLongTaskCount() - longBefore,
    tilesReceived: surface.counters.received - before.received,
    tilesDroppedUnwanted: surface.counters.droppedUnwanted - before.droppedUnwanted,
    tileRequests: surface.counters.requests - before.requests,
  };
}

export async function run(host: HTMLElement): Promise<unknown> {
  const surface = new DocumentSurface(host);
  surface.element.focus();
  const refresh = await measureRefresh();
  const { width, dpr } = surface.viewport;

  const openStart = performance.now();
  const info = await invoke<ViewInfo>("bench_open_synthetic", {
    rows: ROWS,
    rowHeight: ROW_CSS_PX * dpr,
    width,
    channel: surface.createChannel(),
  });
  surface.attach(info);
  const firstPaintMs = performance.now() - openStart + (await untilSharp(surface));
  await nextFrame();
  const snapshots: Record<string, string> = { top: surface.snapshot() };

  const v = (cssPerSecond: number) => (cssPerSecond * dpr) / 1000;
  const steady = await phase(surface, refresh, 6000, (dt) => {
    surface.scrollBy(v(2400) * dt);
  });
  await untilSharp(surface);
  const fling = await phase(surface, refresh, 4000, (dt) => {
    surface.scrollBy(v(24_000) * dt);
  });
  await untilSharp(surface);
  surface.scrollTo(0);
  await untilSharp(surface);
  const drag = await phase(surface, refresh, 3000, (_, elapsed) => {
    surface.scrollTo((Math.min(elapsed, 3000) / 3000) * surface.maxOffset);
  });
  await untilSharp(surface);

  // Deterministic "random" jump targets across the whole document.
  const jumps: number[] = [];
  let seed = 12345;
  for (let i = 0; i < 15; i++) {
    seed = (seed * 1103515245 + 12345) % 2 ** 31;
    surface.scrollTo((seed / 2 ** 31) * surface.maxOffset);
    jumps.push(await untilSharp(surface));
    await sleep(300);
  }
  surface.scrollTo(surface.maxOffset);
  await untilSharp(surface);
  await nextFrame();
  snapshots.end = surface.snapshot();

  const stats = await invoke<unknown>("perf_stats");
  return {
    scenario: "scroll-list",
    env: {
      userAgent: navigator.userAgent,
      devicePixelRatio: dpr,
      viewport: [surface.viewport.width, surface.viewport.height],
      refreshMs: refresh,
      transport: info.transport,
      rows: ROWS,
      extentPx: info.extent.height,
      tileSize: surface.tileSize,
      date: new Date().toISOString(),
    },
    firstPaintMs: Math.round(firstPaintMs * 10) / 10,
    steady,
    fling,
    drag,
    jumpToSharpMs: summarize(jumps),
    perf: stats,
    snapshots,
  };
}
