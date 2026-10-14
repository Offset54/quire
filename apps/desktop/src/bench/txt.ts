// TXT scenarios for the §4 TXT rows (M1 DoD). Run in the bench build by
// tools/bench/refsim-txt.ps1, which picks the fixture (QUIRE_BENCH_FILE),
// purges the file cache for cold runs and checks bench/budgets.json.
//
// - txt-2g: first paint (cold), approximate jumps before indexing, full
//   index, exact jumps, inserts at random offsets (core time), typing
//   latency, scrolling, memory.
// - txt-2g-search: literal search on a cold file, first hit and full pass.
// - txt-save: a small edit in the middle of a 2 GiB copy, then save.
// - txt-300m: the one-line 300 MB JSON: first paint, caret moves, wrap.
// - launch-2g: the app launched with the 2 GiB file (ADR-0022): process
//   creation to first content.

import { invoke } from "@tauri-apps/api/core";
import { getLongTaskCount } from "../perf/trace";
import { summarize } from "../perf/stats";
import { type Loaf, frameWaitStallMs, stallsIn as framesStalls, stallTotal } from "./stalls";
import { getState } from "../txt/doc-store";
import { Session } from "../txt/session";
import type { EditSample, InputSample, TxtView } from "../txt/txt-view";

export const log = (message: string) => {
  void invoke("bench_log", { message });
};

export const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface Env {
  file: string | null;
  phase: string | null;
}

export async function env(): Promise<Env> {
  return invoke<Env>("bench_env");
}

export async function until<T>(
  f: () => T | undefined | false,
  timeoutMs: number,
  what: string,
): Promise<T> {
  log(`waiting: ${what}`);
  const start = performance.now();
  for (;;) {
    const v = f();
    if (v) return v;
    if (performance.now() - start > timeoutMs) throw new Error(`timed out: ${what}`);
    await nextFrame();
  }
}

// Long animation frames, for platform stalls (ADR-0024, ./stalls.ts).
const loafs: Loaf[] = [];
let loafObserver: PerformanceObserver | undefined;

export function watchFrames() {
  if (loafObserver) return;
  loafObserver = new PerformanceObserver((l) => {
    for (const e of l.getEntries()) loafs.push(e.toJSON() as Loaf);
  });
  try {
    loafObserver.observe({ type: "long-animation-frame", buffered: true });
  } catch {
    // not supported: no stalls are subtracted
  }
}

/** Platform stalls inside [from, to] (performance.now() times). */
export function stallsIn(from: number, to: number) {
  for (const e of loafObserver?.takeRecords() ?? []) loafs.push(e.toJSON() as Loaf);
  return framesStalls(loafs, from, to);
}

const longTaskEntries: { startTime: number; duration: number }[] = [];
let longTaskObserver: PerformanceObserver | undefined;

/** Long tasks (start, duration) seen so far, buffered from page start. */
export function longTasksSeen(): { startTime: number; duration: number }[] {
  if (!longTaskObserver) {
    longTaskObserver = new PerformanceObserver((l) => {
      for (const e of l.getEntries())
        longTaskEntries.push({ startTime: e.startTime, duration: e.duration });
    });
    try {
      longTaskObserver.observe({ type: "longtask", buffered: true });
    } catch {
      // not supported
    }
  }
  for (const e of longTaskObserver.takeRecords()) {
    longTaskEntries.push({ startTime: e.startTime, duration: e.duration });
  }
  return longTaskEntries;
}

/** Frames over 50 ms with a blocking task inside [from, to], with their
 *  script attribution (where a long task's time went). */
export function longFramesIn(from: number, to: number): unknown[] {
  stallsIn(0, 0); // collect pending entries
  return loafs
    .filter((f) => f.blockingDuration > 0 && f.startTime < to && f.startTime + f.duration > from)
    .map((f) => {
      const j = f as unknown as {
        scripts: {
          invoker?: string;
          sourceFunctionName?: string;
          sourceURL?: string;
          duration: number;
          forcedStyleAndLayoutDuration?: number;
        }[];
      };
      return {
        atMs: Math.round(f.startTime - from),
        durationMs: Math.round(f.duration),
        blockingMs: Math.round(f.blockingDuration),
        renderMs: f.renderStart > 0 ? Math.round(f.startTime + f.duration - f.renderStart) : 0,
        scripts: j.scripts.map((x) => ({
          invoker: x.invoker,
          fn: x.sourceFunctionName,
          url: x.sourceURL?.split("/").pop(),
          ms: Math.round(x.duration),
          layoutMs: Math.round(x.forcedStyleAndLayoutDuration ?? 0),
        })),
      };
    });
}

export const round1 = (v: number) => Math.round(v * 10) / 10;

/** The platform's part of a wait for a requested frame (ADR-0024). */
export function frameWaitStall(requested: number, drawn: number): number {
  stallsIn(0, 0); // collect pending entries
  return frameWaitStallMs(loafs, requested, drawn);
}

/** From the first rows' arrival to the frame that drew them, and the part of
 * that wait the platform didn't deliver a frame (ADR-0024). */
function firstFrameWait() {
  const ready = performance.getEntriesByName("txt-first-rows-ready", "mark")[0];
  const drawn = performance.getEntriesByName("txt-first-content", "mark")[0];
  if (!ready || !drawn) return { waitMs: null, waitStallMs: 0 };
  return {
    waitMs: round1(drawn.startTime - ready.startTime),
    waitStallMs: frameWaitStall(ready.startTime, drawn.startTime),
  };
}

/** Opens `path` in a view inside `host`; resolves at the first drawn rows. */
async function openView(host: HTMLElement, path: string) {
  watchFrames();
  const t0 = performance.now();
  const session = await Session.open(path);
  if (!session) throw new Error("open failed");
  const openedMs = performance.now() - t0;
  const view = session.attach(host);
  await until(() => view.visibleText().length > 0, 10_000, "first rows");
  await nextFrame();
  const end = performance.now();
  const stalls = stallsIn(t0, end);
  const { waitMs, waitStallMs } = firstFrameWait();
  // The two can describe the same wait: take the larger, never the sum.
  const stallMs = Math.max(stallTotal(stalls), waitStallMs);
  return {
    session,
    view,
    openedMs,
    firstPaintMs: end - t0,
    ownFirstPaintMs: end - t0 - stallMs,
    firstPaintStalls: stalls,
    firstFrameWaitMs: waitMs,
    firstFrameWaitStallMs: waitStallMs,
  };
}

async function indexDone(timeoutMs = 60_000): Promise<number | undefined> {
  const st = await until(
    () => getState().status?.index.complete && getState().status,
    timeoutMs,
    "index",
  );
  return st.indexMs;
}

/** The fixture's line `n` (1-based) starts with n right-aligned to 10 columns. */
function linePrefix(n: number): string {
  return `${String(n).padStart(10, " ")}| `;
}

async function jumps(session: Session, view: TxtView, lines: number[]) {
  const out: {
    line: number;
    ms: number;
    ownMs: number;
    coreMs: number;
    viewCoreMs: number;
    invokeMs: number;
    revealMs: number;
    exact: boolean;
    landed: boolean;
    top: string | undefined;
  }[] = [];
  for (const line of lines) {
    log(`jump to ${line}`);
    const t0 = performance.now();
    const r = await session.gotoLine(line);
    // The last frame wait is a pure wait for a frame; the IPC before it
    // could be our host and is never subtracted (ADR-0024).
    const requested = performance.now();
    await nextFrame();
    const t1 = performance.now();
    const ms = t1 - t0;
    const landed = view.visibleText().some((t) => t.startsWith(linePrefix(line)));
    out.push({
      line,
      ms,
      ownMs: ms - frameWaitStall(requested, t1),
      coreMs: r.ms,
      viewCoreMs: (view.currentHeader?.coreUs ?? 0) / 1000,
      invokeMs: r.invokeMs,
      revealMs: r.revealMs,
      exact: r.exact,
      landed,
      top: view.visibleText()[0]?.slice(0, 24),
    });
  }
  return out;
}

function rand(seed: number) {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) % 2 ** 31;
    return s / 2 ** 31;
  };
}

/** Something that scrolls, for frame measurements. */
export interface Scroller {
  /** Scrolls by `px` CSS pixels. */
  scrollPx(px: number): void;
  /** Whether a placeholder is on screen. */
  showsPlaceholder(): boolean;
}

function txtScroller(view: TxtView, lineH: number): Scroller {
  return {
    scrollPx: (px) => {
      view.scrollByRows(px / lineH);
    },
    showsPlaceholder: () => view.placeholderRows() > 0,
  };
}

async function measureFrames(view: TxtView, durationMs: number, rowsPerSecond: number) {
  return measureScroll(txtScroller(view, 20), durationMs, rowsPerSecond * 20);
}

/** Scrolls `s` at `pxPerSecond` for `durationMs` and measures the frames. */
export async function measureScroll(s: Scroller, durationMs: number, pxPerSecond: number) {
  watchFrames();
  const gaps: [number, number][] = []; // [from, to]
  let blank = 0;
  let frames = 0;
  const long0 = getLongTaskCount();
  const start = await nextFrame();
  let last = start;
  let worst = { gapMs: 0, atMs: 0 };
  while (last - start < durationMs) {
    const t = await nextFrame();
    gaps.push([last, t]);
    if (t - last > worst.gapMs) worst = { gapMs: t - last, atMs: Math.round(last - start) };
    s.scrollPx((pxPerSecond * (t - last)) / 1000);
    frames++;
    if (s.showsPlaceholder()) blank++;
    last = t;
  }
  // Each interval is a wait for a requested frame: without the platform's
  // stall time in it (ADR-0024).
  const perGap = gaps.map(([a, b]) => frameWaitStall(a, b));
  const own = gaps.map(([a, b], i) => b - a - perGap[i]!);
  const stalls = gaps.flatMap(([a, b], i) =>
    perGap[i]! > 0
      ? [{ atMs: Math.round(a - start), gapMs: round1(b - a), stallMs: perGap[i]! }]
      : [],
  );
  // Where a slow frame's time went (script, style and layout, or nothing on
  // the main thread).
  const longFrames = loafs
    .filter((f) => f.startTime < last && f.startTime + f.duration > start && f.duration > 50)
    .slice(0, 5);
  return {
    frameIntervalMs: summarize(gaps.map(([a, b]) => b - a)),
    ownFrameIntervalMs: summarize(own),
    platformStalls: {
      count: stalls.length,
      totalMs: round1(stallTotal(stalls)),
      worstMs: round1(Math.max(0, ...stalls.map((s) => s.stallMs))),
      frames: stalls.slice(0, 10),
    },
    framesOver50ms: gaps.filter(([a, b]) => b - a > 50).length,
    worstFrame: worst,
    longFrames,
    placeholderFramesPct: Math.round((blank / Math.max(1, frames)) * 1000) / 10,
    longTasks: getLongTaskCount() - long0,
  };
}

export async function typing(view: TxtView, count: number, intervalMs: number) {
  const samples: InputSample[] = [];
  const edits: EditSample[] = [];
  const stop = view.onInputSample((s) => samples.push(s));
  const stopEdits = view.onEditSample((s) => edits.push(s));
  const ta = document.querySelector<HTMLTextAreaElement>(".txt-input")!;
  ta.focus();
  const long0 = getLongTaskCount();
  for (let i = 0; i < count; i++) {
    const ch = "abcdefghij"[i % 10]!;
    ta.dispatchEvent(
      new InputEvent("beforeinput", { inputType: "insertText", data: ch, cancelable: true }),
    );
    await sleep(intervalMs);
  }
  await until(() => samples.length >= count * 0.9, 10_000, "typing samples");
  await sleep(500); // the last confirmations
  stop();
  stopEdits();
  const checked = edits.filter((e) => e.echoMatched !== undefined);
  return {
    // Keystroke to the first frame showing it (a local echo when possible).
    latencyMs: summarize(samples.map((s) => s.latencyMs)),
    echoed: samples.filter((s) => s.echo).length,
    // Per request to the core: to the frame with its result, and the parts.
    confirmMs: summarize(edits.map((s) => s.confirmMs)),
    roundTripMs: summarize(edits.map((s) => s.roundTripMs)),
    waitMs: summarize(edits.map((s) => s.waitMs)),
    adoptMs: summarize(edits.map((s) => s.adoptMs)),
    coreMs: summarize(edits.map((s) => s.coreMs)),
    echoChecks: checked.length,
    echoMismatches: checked.filter((e) => !e.echoMatched).length,
    samples: samples.length,
    requests: edits.length,
    longTasks: getLongTaskCount() - long0,
  };
}

export async function memory(): Promise<{ privateBytes: number; workingSet: number } | null> {
  return invoke("bench_memory");
}

export async function txt2g(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const mem0 = await memory();
  const {
    session,
    view,
    openedMs,
    firstPaintMs,
    ownFirstPaintMs,
    firstPaintStalls,
    firstFrameWaitMs,
    firstFrameWaitStallMs,
  } = await openView(host, e.file);
  const totalLines = 1_662_532; // from the fixture manifest; checked below
  const r = rand(42);
  // Before the index completes: approximate jumps (§4: ≤ 300 ms).
  const early = getState().status?.index.complete
    ? []
    : await jumps(
        session,
        view,
        Array.from({ length: 5 }, () => 1 + Math.floor(r() * totalLines)),
      );
  const indexMs = await indexDone();
  const exactLines = getState().status?.lineCount;
  const indexed = await jumps(
    session,
    view,
    Array.from({ length: 30 }, () => 1 + Math.floor(r() * totalLines)),
  );

  // Inserts at random offsets, core time.
  const len = getState().status?.len ?? 0;
  const offsets = Array.from({ length: 200 }, () => Math.floor(r() * len));
  const insertUs = await invoke<number[]>("bench_txt_insert", {
    doc: session.doc,
    offsets,
    text: "x",
  });
  view.refresh();
  await nextFrame();

  // Typing in the middle of the file.
  await session.gotoLine(Math.floor(totalLines / 2));
  const type = await typing(view, 300, 30);

  // Scrolling: steady (2,400 CSS px/s) and fling (24,000 CSS px/s).
  const lineH = 20;
  const steady = await measureFrames(view, 5000, 2400 / lineH);
  const fling = await measureFrames(view, 3000, 24_000 / lineH);
  // Scrolling that starts after the screen has been static for a while:
  // the worst frame of a short scroll after each idle period.
  const scrollAfterIdle: { idleMs: number; worstMs: number; ownWorstMs: number }[] = [];
  for (const idleMs of [100, 300, 500, 700, 1000, 2000, 300]) {
    await sleep(idleMs);
    const m = await measureFrames(view, 250, 2400 / lineH);
    scrollAfterIdle.push({
      idleMs,
      worstMs: round1(m.worstFrame.gapMs),
      ownWorstMs: m.ownFrameIntervalMs.max,
    });
  }
  const mem1 = await memory();
  await session.discard();
  return {
    scenario: "txt-2g",
    file: e.file,
    firstPaintMs: round1(firstPaintMs),
    ownFirstPaintMs: round1(ownFirstPaintMs),
    firstPaintStalls,
    firstFrameWaitMs,
    firstFrameWaitStallMs,
    openMs: Math.round(openedMs * 10) / 10,
    indexMs,
    lineCount: exactLines,
    expectedLines: totalLines,
    jumpBeforeIndex: {
      ms: summarize(early.map((j) => j.ms)),
      ownMs: summarize(early.map((j) => j.ownMs)),
      runs: early,
    },
    jumpIndexed: {
      ms: summarize(indexed.map((j) => j.ms)),
      ownMs: summarize(indexed.map((j) => j.ownMs)),
      allLanded: indexed.every((j) => j.landed && j.exact),
      runs: indexed,
    },
    insertMs: summarize(insertUs.map((u) => u / 1000)),
    typing: type,
    scrollAfterIdle,
    steady,
    fling,
    memory: {
      before: mem0,
      after: mem1,
      privateGrowthMiB:
        mem0 && mem1 ? Math.round((mem1.privateBytes - mem0.privateBytes) / 1048576) : null,
    },
  };
}

export async function txt2gSearch(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const { session } = await openView(host, e.file);
  const t0 = performance.now();
  await session.search({
    pattern: "kerning budget",
    regex: false,
    caseSensitive: true,
    wholeWord: false,
  });
  const first = await until(
    () => (getState().search?.matches ?? 0) > 0 && performance.now(),
    60_000,
    "first hit",
  );
  const done = await until(
    () => getState().search?.complete && getState().search,
    120_000,
    "search done",
  );
  const totalMs = performance.now() - t0;
  const indexComplete = getState().status?.index.complete;
  await session.discard();
  return {
    scenario: "txt-2g-search",
    pattern: "kerning budget",
    firstHitMs: Math.round((first - t0) * 10) / 10,
    coreFirstHitMs: done.firstHitMs,
    totalMs: Math.round(totalMs * 10) / 10,
    coreMs: done.ms,
    matches: done.matches,
    truncated: done.truncated,
    indexCompleteAtEnd: indexComplete,
  };
}

export async function txtSave(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const { session, view } = await openView(host, e.file);
  await indexDone();
  await session.gotoLine(800_000);
  view.send({ type: "type", text: "SAVE-BENCH " });
  await until(() => getState().status?.dirty, 5000, "dirty");
  const t0 = performance.now();
  const ok = await session.save(false);
  const ms = performance.now() - t0;
  const bytes = getState().status?.len ?? 0;
  // The core's own timing of the save and its phases (the rest is IPC and
  // the frontend).
  const core = performance.getEntriesByName("txt-save", "measure").at(-1) as
    PerformanceMeasure | undefined;
  await session.discard();
  return {
    scenario: "txt-save",
    ok,
    saveMs: Math.round(ms),
    bytes,
    core: (core?.detail as unknown) ?? null,
  };
}

export async function txt300m(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const {
    session,
    view,
    openedMs,
    firstPaintMs,
    ownFirstPaintMs,
    firstPaintStalls,
    firstFrameWaitMs,
    firstFrameWaitStallMs,
  } = await openView(host, e.file);
  const firstViewCoreMs = (view.currentHeader?.coreUs ?? 0) / 1000;
  const long0 = getLongTaskCount();
  const coreMs: Record<string, number> = {};
  const timeAction = async (a: Parameters<TxtView["send"]>[0]) => {
    const samples: InputSample[] = [];
    const edits: EditSample[] = [];
    const stop = view.onInputSample((s) => samples.push(s));
    const stopEdits = view.onEditSample((s) => edits.push(s));
    view.send(a);
    await until(() => samples.length > 0 && edits.length > 0, 10_000, `response to ${a.type}`);
    stop();
    stopEdits();
    coreMs[`${Object.keys(coreMs).length}:${a.type}`] = edits[0]!.coreMs;
    return samples[0]!.latencyMs;
  };
  const moves: Record<string, number> = {};
  moves.end = await timeAction({ type: "move", to: "end", extend: false });
  moves.home = await timeAction({ type: "move", to: "home", extend: false });
  moves.docEnd = await timeAction({ type: "move", to: "docEnd", extend: false });
  moves.wordLeft = await timeAction({ type: "move", to: "wordLeft", extend: false });
  moves.left = await timeAction({ type: "move", to: "left", extend: false });
  moves.selectToStart = await timeAction({ type: "move", to: "docStart", extend: true });
  moves.collapse = await timeAction({ type: "collapse" });
  moves.typeAtEnd = await timeAction({ type: "type", text: "x" });
  // Wrap on: the 300 MB line becomes millions of rows; then off again.
  const wrapToggle = async (on: boolean) => {
    const t0 = performance.now();
    view.setWrap(on);
    await until(() => view.visibleText().length > 0 && performance.now() - t0 > 16, 10_000, "rows");
    await nextFrame();
    return performance.now() - t0;
  };
  const wrapOn = await wrapToggle(true);
  const wrapped = await measureFrames(view, 2000, -2400 / 20);
  const wrapOff = await wrapToggle(false);
  const moveMs = Object.values(moves);
  await session.discard();
  return {
    scenario: "txt-300m",
    firstPaintMs: round1(firstPaintMs),
    ownFirstPaintMs: round1(ownFirstPaintMs),
    firstPaintStalls,
    firstFrameWaitMs,
    firstFrameWaitStallMs,
    openedMs: Math.round(openedMs * 10) / 10,
    firstViewCoreMs,
    caretMs: { max: Math.max(...moveMs), each: moves, coreMs },
    wrapToggleMs: { on: wrapOn, off: wrapOff, max: Math.max(wrapOn, wrapOff) },
    wrappedScroll: wrapped,
    longTasks: getLongTaskCount() - long0,
  };
}

/** Launched with the file as an argument: the app shows it on its own. */
export async function launch2g(): Promise<unknown> {
  watchFrames();
  const mark = await until(
    () => performance.getEntriesByName("txt-first-content", "mark")[0],
    30_000,
    "first content",
  );
  // Before the page's time origin the webview wasn't running; stalls are
  // only seen from there on.
  const stalls = stallsIn(0, mark.startTime);
  const { waitMs, waitStallMs } = firstFrameWait();
  return {
    scenario: "launch-2g",
    timeOriginEpochMs: performance.timeOrigin,
    firstContentEpochMs: performance.timeOrigin + mark.startTime,
    firstFrameWaitMs: waitMs,
    stallMs: round1(Math.max(stallTotal(stalls), waitStallMs)),
    stalls,
  };
}
