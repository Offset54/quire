// Markdown scenarios for the §4 Markdown row and the cross-cutting rows
// (M2 DoD). Run in the bench build by tools/bench/refsim-md.ps1, which
// purges the fixture from the file cache for cold runs and checks the "md"
// budgets in bench/budgets.json.
//
// - md-20m: the 20 MiB fixture opened cold in split view: first paint of
//   both panes (source rows drawn, and the preview's visible blocks
//   rendered), the background index, memory.
// - md-20m-edit: keystroke → preview update at the plan's positions (start,
//   middle, inside the 1,500-row table, inside a list, a reference
//   definition used far away), typing latency in the source view, preview
//   scrolling steady and fling.

import { invoke } from "@tauri-apps/api/core";
import { getMdStatus, type MdController } from "../md/controller";
import type { PatchSample } from "../md/preview";
import { summarize } from "../perf/stats";
import { getLongTaskCount } from "../perf/trace";
import { getState } from "../txt/doc-store";
import { Session } from "../txt/session";
import type { TxtView } from "../txt/txt-view";
import { stallTotal } from "./stalls";
import {
  env,
  frameWaitStall,
  log,
  longFramesIn,
  longTasksSeen,
  measureScroll,
  memory,
  nextFrame,
  round1,
  sleep,
  stallsIn,
  typing,
  until,
  watchFrames,
} from "./txt";

/** The editor's pane layout (as EditorScreen builds it) inside `host`. */
function panes(host: HTMLElement) {
  const root = document.createElement("div");
  root.className = "editor-panes markdown";
  root.style.position = "absolute";
  root.style.inset = "0";
  const source = document.createElement("div");
  source.className = "editor-surface";
  const previewHost = document.createElement("div");
  previewHost.className = "md-preview-host";
  root.append(source, previewHost);
  host.append(root);
  return { root, source, previewHost };
}

/** Opens `path` in split view; resolves at the frame that shows both panes. */
async function openSplit(host: HTMLElement, path: string) {
  try {
    localStorage.setItem("quire.md.mode", "split");
  } catch {
    // The default mode would be the editing view; the check below fails.
  }
  watchFrames();
  longTasksSeen();
  const long0 = getLongTaskCount();
  const t0 = performance.now();
  const session = await Session.open(path);
  if (!session) throw new Error("open failed");
  const openedMs = performance.now() - t0;
  const { root, source, previewHost } = panes(host);
  const view = session.attach(source, { root, previewHost });
  const md = await until(() => session.md, 10_000, "markdown panes");
  if (md.mode !== "split") throw new Error(`not in split view (${md.mode})`);
  // Both panes show content: source rows drawn, preview blocks on screen
  // with their HTML (not placeholders).
  let ready = 0;
  await until(
    () => {
      const ok =
        view.visibleText().length > 0 &&
        md.preview.visibleBlocks().some((b) => b.text.length > 0) &&
        md.preview.pendingBlocks() === 0;
      if (ok && ready === 0) ready = performance.now();
      return ok;
    },
    10_000,
    "both panes",
  );
  const end = performance.now();
  const stalls = stallsIn(t0, end);
  // The wait from content being ready to the frame that showed it.
  const waitStallMs = frameWaitStall(ready, end);
  const stallMs = Math.max(stallTotal(stalls), waitStallMs);
  return {
    session,
    view,
    md,
    openedMs,
    firstPaintMs: end - t0,
    ownFirstPaintMs: end - t0 - stallMs,
    firstPaintStalls: stalls,
    firstFrameWaitStallMs: waitStallMs,
    longTasks: getLongTaskCount() - long0,
    longFrames: longFramesIn(t0, end),
    longTaskTimes: longTasksSeen().map((x) => ({
      atMs: Math.round(x.startTime - t0),
      ms: Math.round(x.duration),
    })),
  };
}

async function indexed(timeoutMs = 60_000): Promise<number> {
  const t0 = performance.now();
  await until(() => getMdStatus()?.indexed, timeoutMs, "markdown index");
  const m = performance.getEntriesByName("md-index", "measure").at(-1);
  return m ? m.duration : performance.now() - t0;
}

/** The first match of `pattern` at or after `from` (the core's find). */
async function find(session: Session, pattern: string, from: number): Promise<number> {
  session.view?.send({ type: "setSelections", ranges: [[from, from]] });
  await sleep(50);
  const hit = await invoke<[number, number] | null>("txt_find", {
    doc: session.doc,
    query: { pattern, regex: false, caseSensitive: true, wholeWord: false },
    backwards: false,
    replace: null,
  });
  if (!hit) throw new Error(`not found: ${pattern}`);
  return hit[0];
}

interface EditTiming {
  /** Keystroke to the frame that shows the pushed window with the edit. */
  ms: number;
  /** Keystroke to the frame with the table corrections, when there were any. */
  correctionsMs: number | null;
}

/** Types `count` characters at `offset` and times each until the preview shows it. */
async function keystrokeToPreview(
  view: TxtView,
  md: MdController,
  offset: number,
  count: number,
): Promise<EditTiming[]> {
  view.send({ type: "setSelections", ranges: [[offset, offset]] });
  await view.reveal(offset);
  await md.preview.reveal(offset);
  await sleep(300);
  const ta = document.querySelector<HTMLTextAreaElement>(".txt-input")!;
  ta.focus();
  const out: EditTiming[] = [];
  for (let i = 0; i < count; i++) {
    const rev0 = md.preview.revision;
    const patches: PatchSample[] = [];
    const stop = md.preview.onPatch((p) => patches.push(p));
    const t0 = performance.now();
    ta.dispatchEvent(
      new InputEvent("beforeinput", { inputType: "insertText", data: "x", cancelable: true }),
    );
    const first = await until(
      () => patches.find((p) => p.revision > rev0),
      5000,
      `preview update ${i} at ${offset}`,
    );
    // Corrections (table rebuild) arrive as a second window, if at all.
    await sleep(120);
    stop();
    const later = patches.filter((p) => p !== first && p.revision === first.revision);
    out.push({
      ms: round1(first.painted - t0),
      correctionsMs: later.length > 0 ? round1(later[later.length - 1]!.painted - t0) : null,
    });
  }
  return out;
}

export async function md20m(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const mem0 = await memory();
  const o = await openSplit(host, e.file);
  log(`first paint ${round1(o.firstPaintMs)} ms`);
  const indexMs = await indexed();
  const mem1 = await memory();
  const status = await invoke<Record<string, unknown>>("md_status", { doc: o.session.doc });
  await o.session.discard();
  return {
    scenario: "md-20m",
    file: e.file,
    firstPaintMs: round1(o.firstPaintMs),
    ownFirstPaintMs: round1(o.ownFirstPaintMs),
    firstPaintStalls: o.firstPaintStalls,
    firstFrameWaitStallMs: o.firstFrameWaitStallMs,
    openMs: round1(o.openedMs),
    firstPaintLongTasks: o.longTasks,
    firstPaintLongFrames: o.longFrames,
    longTaskTimes: o.longTaskTimes,
    indexMs: round1(indexMs),
    status,
    memory: {
      before: mem0,
      after: mem1,
      privateGrowthMiB:
        mem0 && mem1 ? Math.round((mem1.privateBytes - mem0.privateBytes) / 1048576) : null,
    },
  };
}

export async function md20mEdit(host: HTMLElement): Promise<unknown> {
  const e = await env();
  if (!e.file) throw new Error("QUIRE_BENCH_FILE not set");
  const { session, view, md } = await openSplit(host, e.file);
  await indexed();
  const len = getState().status?.len ?? 0;
  const long0 = getLongTaskCount();

  // The plan's positions (M2_PLAN, effective budgets).
  const positions: [string, number][] = [];
  positions.push(["start", (await find(session, "Section 0:", 0)) + 300]);
  positions.push(["middle", (await find(session, "Line with a hard break", len / 2)) + 5]);
  const big = await find(session, "Section 8000:", 0);
  positions.push(["bigTable", (await find(session, "|:---|:---:|---:|", big)) + 20_000]);
  positions.push(["list", (await find(session, "- first item", len * 0.3)) + 4]);
  positions.push([
    "referenceDefinition",
    (await find(session, "[ref-5]: https://", len * 0.9)) + 12,
  ]);

  const edits: Record<
    string,
    {
      ms: ReturnType<typeof summarize>;
      correctionsMs: ReturnType<typeof summarize> | null;
      runs: EditTiming[];
    }
  > = {};
  const all: number[] = [];
  for (const [name, offset] of positions) {
    log(`keystroke → preview at ${name} (${offset})`);
    const runs = await keystrokeToPreview(view, md, offset, 20);
    const corr = runs.flatMap((r) => (r.correctionsMs === null ? [] : [r.correctionsMs]));
    edits[name] = {
      ms: summarize(runs.map((r) => r.ms)),
      correctionsMs: corr.length > 0 ? summarize(corr) : null,
      runs,
    };
    all.push(...runs.map((r) => r.ms));
  }

  // Typing in the source view (split, the preview following).
  await view.reveal(Math.floor(len / 3));
  const type = await typing(view, 300, 30);

  // Preview scrolling: steady (2,400 CSS px/s) and fling (24,000 CSS px/s).
  await md.preview.reveal(Math.floor(len / 4));
  await sleep(500);
  md.preview.focus();
  const scroller = {
    scrollPx: (px: number) => {
      md.preview.scrollBy(px);
    },
    showsPlaceholder: () => md.preview.pendingBlocks() > 0,
  };
  const steady = await measureScroll(scroller, 5000, 2400);
  const fling = await measureScroll(scroller, 3000, 24_000);
  await nextFrame();
  const longTasks = getLongTaskCount() - long0;
  await session.discard();
  return {
    scenario: "md-20m-edit",
    file: e.file,
    keystrokeToPreview: { ms: summarize(all), positions: edits },
    typing: type,
    previewSteady: steady,
    previewFling: fling,
    longTasks,
  };
}
