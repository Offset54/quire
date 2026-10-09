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
import { getState } from "../txt/doc-store";
import { Session } from "../txt/session";
import type { EditSample, InputSample, TxtView } from "../txt/txt-view";

const log = (message: string) => {
  void invoke("bench_log", { message });
};

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface Env {
  file: string | null;
  phase: string | null;
}

async function env(): Promise<Env> {
  return invoke<Env>("bench_env");
}

async function until<T>(
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

/** Opens `path` in a view inside `host`; resolves at the first drawn rows. */
async function openView(host: HTMLElement, path: string) {
  const t0 = performance.now();
  const session = await Session.open(path);
  if (!session) throw new Error("open failed");
  const openedMs = performance.now() - t0;
  const view = session.attach(host);
  await until(() => view.visibleText().length > 0, 10_000, "first rows");
  await nextFrame();
  return { session, view, openedMs, firstPaintMs: performance.now() - t0 };
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
    await nextFrame();
    const ms = performance.now() - t0;
    const landed = view.visibleText().some((t) => t.startsWith(linePrefix(line)));
    out.push({
      line,
      ms,
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

async function measureFrames(view: TxtView, durationMs: number, rowsPerSecond: number) {
  const gaps: number[] = [];
  let blank = 0;
  let frames = 0;
  const long0 = getLongTaskCount();
  // Long animation frames: where a slow frame's time went (script, style
  // and layout, or nothing on the main thread).
  const loafs: unknown[] = [];
  const po = new PerformanceObserver((l) => {
    for (const e of l.getEntries()) {
      if (e.duration > 50) loafs.push(e.toJSON());
    }
  });
  try {
    po.observe({ type: "long-animation-frame", buffered: false });
  } catch {
    // not supported
  }
  const start = await nextFrame();
  let last = start;
  let worst = { gapMs: 0, atMs: 0 };
  while (last - start < durationMs) {
    const t = await nextFrame();
    gaps.push(t - last);
    if (t - last > worst.gapMs) worst = { gapMs: t - last, atMs: Math.round(last - start) };
    view.scrollByRows((rowsPerSecond * (t - last)) / 1000);
    frames++;
    if (view.placeholderRows() > 0) blank++;
    last = t;
  }
  po.disconnect();
  return {
    frameIntervalMs: summarize(gaps),
    framesOver50ms: gaps.filter((g) => g > 50).length,
    worstFrame: worst,
    longFrames: loafs.slice(0, 5),
    placeholderFramesPct: Math.round((blank / Math.max(1, frames)) * 1000) / 10,
    longTasks: getLongTaskCount() - long0,
  };
}

async function typing(view: TxtView, count: number, intervalMs: number) {
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

async function memory(): Promise<{ privateBytes: number; workingSet: number } | null> {
  return invoke("bench_memory");
}

export async function txt2g(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const mem0 = await memory();
  const { session, view, openedMs, firstPaintMs } = await openView(host, e.file);
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
  const scrollAfterIdle: { idleMs: number; worstMs: number }[] = [];
  for (const idleMs of [100, 300, 500, 700, 1000, 2000, 300]) {
    await sleep(idleMs);
    const m = await measureFrames(view, 250, 2400 / lineH);
    scrollAfterIdle.push({ idleMs, worstMs: Math.round(m.worstFrame.gapMs * 10) / 10 });
  }
  const mem1 = await memory();
  await session.discard();
  return {
    scenario: "txt-2g",
    file: e.file,
    firstPaintMs: Math.round(firstPaintMs * 10) / 10,
    openMs: Math.round(openedMs * 10) / 10,
    indexMs,
    lineCount: exactLines,
    expectedLines: totalLines,
    jumpBeforeIndex: { ms: summarize(early.map((j) => j.ms)), runs: early },
    jumpIndexed: {
      ms: summarize(indexed.map((j) => j.ms)),
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
  await session.discard();
  return { scenario: "txt-save", ok, saveMs: Math.round(ms), bytes };
}

export async function txt300m(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const { session, view, openedMs, firstPaintMs } = await openView(host, e.file);
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
    firstPaintMs: Math.round(firstPaintMs * 10) / 10,
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
  const mark = await until(
    () => performance.getEntriesByName("txt-first-content", "mark")[0],
    30_000,
    "first content",
  );
  return {
    scenario: "launch-2g",
    timeOriginEpochMs: performance.timeOrigin,
    firstContentEpochMs: performance.timeOrigin + mark.startTime,
  };
}
