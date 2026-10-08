// Tile transport spike runner (M0 task 7, ADR-0004). Runs every scenario,
// then sends the results to the `report` command, which writes them to disk
// and exits the app.

import { Channel, invoke } from "@tauri-apps/api/core";
import { decodeLz4, decodeQoi } from "./decoders";

type Encoding = "raw" | "lz4" | "qoi";
type Content = "text" | "photo";
type Transport = "uri" | "invoke" | "channel" | "shared";

interface TileReq {
  encoding: Encoding;
  content: Content;
  size: number;
  index: number;
}

interface Fetched {
  bytes: Uint8Array;
  release?: () => void;
}

interface SharedBufferEvent {
  getBuffer(): ArrayBuffer;
  additionalData: { id: number };
}

interface WebView2 {
  addEventListener(type: "sharedbufferreceived", listener: (e: SharedBufferEvent) => void): void;
  releaseBuffer(buffer: ArrayBuffer): void;
}

const webview2 = (window as unknown as { chrome?: { webview?: WebView2 } }).chrome?.webview;

const SIZES: number[] = [256, 512];
const ENCODINGS: Encoding[] = ["raw", "lz4", "qoi"];
const CONTENTS: Content[] = ["text", "photo"];
const TRANSPORTS: Transport[] = ["uri", "invoke", "channel", "shared"];
const LATENCY = { warmup: 10, iterations: 80 };
const BURST = { warmup: 2, repetitions: 15 };

const statusEl = document.getElementById("status") as HTMLPreElement;
const canvas = document.getElementById("surface") as HTMLCanvasElement;
const dpr = window.devicePixelRatio;
canvas.width = Math.round(window.innerWidth * dpr);
canvas.height = Math.round(window.innerHeight * dpr);
const ctx = canvas.getContext("2d", { alpha: false })!;

// ---- transports ------------------------------------------------------------

let nextId = 1;
const pending = new Map<number, (f: Fetched) => void>();

const channel = new Channel<ArrayBuffer>();
channel.onmessage = (buf) => {
  const u = new Uint8Array(buf);
  const id = (u[0]! | (u[1]! << 8) | (u[2]! << 16) | (u[3]! << 24)) >>> 0;
  pending.get(id)?.({ bytes: u.subarray(4) });
  pending.delete(id);
};

webview2?.addEventListener("sharedbufferreceived", (e) => {
  const buffer = e.getBuffer();
  const id = e.additionalData.id;
  pending.get(id)?.({
    bytes: new Uint8Array(buffer),
    release: () => {
      webview2.releaseBuffer(buffer);
    },
  });
  pending.delete(id);
});

function viaPush(command: string, req: TileReq, extra: Record<string, unknown>): Promise<Fetched> {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, resolve);
    invoke(command, { req, id, ...extra }).catch((err: unknown) => {
      pending.delete(id);
      reject(err instanceof Error ? err : new Error(String(err)));
    });
  });
}

async function fetchTile(t: Transport, req: TileReq): Promise<Fetched> {
  switch (t) {
    case "uri": {
      const url = `http://tile.localhost/${req.encoding}/${req.content}/${req.size}/${req.index}`;
      const res = await fetch(url);
      return { bytes: new Uint8Array(await res.arrayBuffer()) };
    }
    case "invoke":
      return { bytes: new Uint8Array(await invoke<ArrayBuffer>("tile_invoke", { req })) };
    case "channel":
      return viaPush("tile_channel", req, { channel });
    case "shared":
      return viaPush("tile_shared", req, {});
  }
}

function decode(encoding: Encoding, bytes: Uint8Array, size: number): ImageData {
  let rgba: Uint8Array;
  if (encoding === "raw") rgba = bytes;
  else if (encoding === "lz4") rgba = decodeLz4(bytes);
  else rgba = decodeQoi(bytes).data;
  const clamped = new Uint8ClampedArray(rgba.buffer, rgba.byteOffset, size * size * 4);
  return new ImageData(clamped as Uint8ClampedArray<ArrayBuffer>, size, size);
}

interface Stages {
  transport: number;
  decode: number;
  upload: number;
  total: number;
}

/** Fetches, decodes and draws one tile, returning stage timings in ms. */
async function deliver(t: Transport, req: TileReq, x: number, y: number): Promise<Stages> {
  const t0 = performance.now();
  const fetched = await fetchTile(t, req);
  const t1 = performance.now();
  const image = decode(req.encoding, fetched.bytes, req.size);
  const t2 = performance.now();
  const bitmap = await createImageBitmap(image);
  fetched.release?.();
  const t3 = performance.now();
  ctx.drawImage(bitmap, x, y);
  bitmap.close();
  return { transport: t1 - t0, decode: t2 - t1, upload: t3 - t2, total: t3 - t0 };
}

// ---- measurement helpers ----------------------------------------------------

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return NaN;
  const i = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
  return sorted[Math.max(0, i)]!;
}

function summarize(values: number[]) {
  const s = [...values].sort((a, b) => a - b);
  const round = (v: number) => Math.round(v * 1000) / 1000;
  return {
    n: s.length,
    p50: round(percentile(s, 50)),
    p95: round(percentile(s, 95)),
    max: round(s[s.length - 1] ?? NaN),
    mean: round(s.reduce((a, b) => a + b, 0) / s.length),
  };
}

/** Records frame gaps while a scenario runs, to see whether delivery janks the UI. */
function frameMonitor() {
  const gaps: number[] = [];
  let last = performance.now();
  let running = true;
  const tick = (now: number) => {
    gaps.push(now - last);
    last = now;
    if (running) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  return () => {
    running = false;
    const s = summarize(gaps);
    return { frames: s.n, maxGapMs: s.max, over20ms: gaps.filter((g) => g > 20).length };
  };
}

let longTasks = 0;
new PerformanceObserver((list) => {
  longTasks += list.getEntries().length;
}).observe({ type: "longtask", buffered: true });

const nextFrame = () => new Promise((r) => requestAnimationFrame(r));

// ---- scenarios ----------------------------------------------------------------

async function verifyDecoders(): Promise<Record<string, boolean>> {
  const result: Record<string, boolean> = {};
  for (const size of SIZES) {
    for (const content of CONTENTS) {
      const raw = (await fetchTile("invoke", { encoding: "raw", content, size, index: 3 })).bytes;
      for (const encoding of ["lz4", "qoi"] satisfies Encoding[]) {
        const enc = (await fetchTile("invoke", { encoding, content, size, index: 3 })).bytes;
        const decoded = decode(encoding, enc, size).data;
        result[`${encoding}/${content}/${size}`] =
          decoded.length === raw.length && decoded.every((v, i) => v === raw[i]);
      }
    }
  }
  return result;
}

async function latency(t: Transport, encoding: Encoding, content: Content, size: number) {
  const stages: Stages[] = [];
  const stop = frameMonitor();
  const before = longTasks;
  for (let i = 0; i < LATENCY.warmup + LATENCY.iterations; i++) {
    const s = await deliver(t, { encoding, content, size, index: i }, 0, 0);
    if (i >= LATENCY.warmup) stages.push(s);
  }
  return {
    transport: t,
    encoding,
    content,
    size,
    totalMs: summarize(stages.map((s) => s.total)),
    transportMs: summarize(stages.map((s) => s.transport)),
    decodeMs: summarize(stages.map((s) => s.decode)),
    uploadMs: summarize(stages.map((s) => s.upload)),
    frames: stop(),
    longTasks: longTasks - before,
  };
}

/** A full-viewport repaint: every tile requested at once. */
async function burst(t: Transport, encoding: Encoding, content: Content, size: number) {
  const cols = Math.ceil(canvas.width / size);
  const rows = Math.ceil(canvas.height / size);
  const times: number[] = [];
  const stop = frameMonitor();
  const before = longTasks;
  for (let rep = 0; rep < BURST.warmup + BURST.repetitions; rep++) {
    const t0 = performance.now();
    const jobs: Promise<Stages>[] = [];
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const index = rep * cols * rows + r * cols + c;
        jobs.push(deliver(t, { encoding, content, size, index }, c * size, r * size));
      }
    }
    await Promise.all(jobs);
    if (rep >= BURST.warmup) times.push(performance.now() - t0);
    await nextFrame();
  }
  const tiles = cols * rows;
  const summary = summarize(times);
  const rgbaMB = (tiles * size * size * 4) / 1e6;
  return {
    transport: t,
    encoding,
    content,
    size,
    tiles,
    viewport: [canvas.width, canvas.height],
    repaintMs: summary,
    rgbaMBps: Math.round(rgbaMB / (summary.p50 / 1000)),
    frames: stop(),
    longTasks: longTasks - before,
  };
}

async function run() {
  const transports = TRANSPORTS.filter((t) => t !== "shared" || webview2 !== undefined);
  const env = {
    userAgent: navigator.userAgent,
    devicePixelRatio: dpr,
    canvas: [canvas.width, canvas.height],
    hardwareConcurrency: navigator.hardwareConcurrency,
    transports,
    date: new Date().toISOString(),
  };
  statusEl.textContent = "verifying decoders…";
  const decodersVerified = await verifyDecoders();

  const latencyResults = [];
  const burstResults = [];
  const combos = transports.flatMap((t) =>
    ENCODINGS.flatMap((e) => CONTENTS.flatMap((c) => SIZES.map((s) => [t, e, c, s] as const))),
  );
  let done = 0;
  for (const [t, e, c, s] of combos) {
    statusEl.textContent = `${++done}/${combos.length}  ${t} ${e} ${c} ${s}px`;
    await nextFrame();
    latencyResults.push(await latency(t, e, c, s));
    burstResults.push(await burst(t, e, c, s));
  }
  statusEl.textContent = "reporting…";
  await invoke("report", {
    results: { env, decodersVerified, latency: latencyResults, burst: burstResults },
  });
}

run().catch((err: unknown) => {
  statusEl.textContent = `failed: ${String(err)}`;
  void invoke("report", { results: { error: String(err) } });
});
