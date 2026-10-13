// Platform frame stalls (RISKS #12, ADR-0024). While the page keeps a frame
// requested, a long animation frame with no blocking task and less than one
// 60 Hz frame of our own main-thread work is time the platform didn't deliver
// a frame. Rows are judged on the measured time minus that stall time; raw
// values and the stalls themselves are reported next to them.

export const FRAME_MS = 1000 / 60;

/** The fields of a long-animation-frame entry (its toJSON()) used here. */
export interface Loaf {
  startTime: number;
  duration: number;
  blockingDuration: number;
  renderStart: number;
  scripts: { startTime: number; duration: number }[];
}

export interface Stall {
  atMs: number;
  frameMs: number;
  ownWorkMs: number;
  stallMs: number;
}

/** Our main-thread work in a long frame: scripts before rendering, then rendering. */
export function ownWorkMs(f: Loaf): number {
  const end = f.startTime + f.duration;
  const render = f.renderStart > 0 ? end - f.renderStart : 0;
  const before = f.scripts
    .filter((s) => f.renderStart === 0 || s.startTime < f.renderStart)
    .reduce((sum, s) => sum + s.duration, 0);
  return before + render;
}

/**
 * Time in a long frame the platform didn't deliver a frame: the idle time
 * before rendering started (the frame's start to `renderStart`, less our
 * scripts in between), minus one frame. Our rendering work comes after that
 * wait and can't have caused it. 0 if a task blocked in the frame.
 */
export function frameStallMs(f: Loaf): number {
  if (f.blockingDuration > 0) return 0;
  const renderAt = f.renderStart > 0 ? f.renderStart : f.startTime + f.duration;
  const scriptsBefore = f.scripts
    .filter((s) => s.startTime < renderAt)
    .reduce((sum, s) => sum + s.duration, 0);
  return Math.max(0, renderAt - f.startTime - scriptsBefore - FRAME_MS);
}

const round1 = (v: number) => Math.round(v * 10) / 10;

/** Platform stalls of `frames` inside [from, to] (performance.now() times). */
export function stallsIn(frames: readonly Loaf[], from: number, to: number): Stall[] {
  const out: Stall[] = [];
  for (const f of frames) {
    const overlap = Math.min(to, f.startTime + f.duration) - Math.max(from, f.startTime);
    const stall = Math.min(frameStallMs(f), overlap);
    if (stall > 0) {
      out.push({
        atMs: Math.round(f.startTime),
        frameMs: round1(f.duration),
        ownWorkMs: round1(ownWorkMs(f)),
        stallMs: round1(stall),
      });
    }
  }
  return out;
}

/**
 * A wait for one requested frame, from `requested` to `drawn`. The page
 * renders as soon as a frame is delivered between tasks, and while a frame
 * is pending every task falls inside a long animation frame once the wait
 * passes 50 ms. So the wait, less all our work in the long frames that
 * overlap it, less one frame, is the platform not delivering the frame.
 * (Idle time before a long frame's first task is in no entry, which is why
 * [`stallsIn`] alone misses part of such waits.) 0 if a task blocked in
 * the window. Use only for spans that are pure frame waits, never for
 * spans that also wait for the host.
 */
export function frameWaitStallMs(
  frames: readonly Loaf[],
  requested: number,
  drawn: number,
): number {
  let work = 0;
  for (const f of frames) {
    if (f.startTime >= drawn || f.startTime + f.duration <= requested) continue;
    if (f.blockingDuration > 0) return 0;
    work += ownWorkMs(f);
  }
  return round1(Math.max(0, drawn - requested - work - FRAME_MS));
}

export const stallTotal = (s: readonly { stallMs: number }[]) =>
  s.reduce((sum, x) => sum + x.stallMs, 0);
