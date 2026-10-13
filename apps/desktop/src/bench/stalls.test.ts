import { describe, expect, it } from "vitest";
import {
  FRAME_MS,
  type Loaf,
  frameStallMs,
  frameWaitStallMs,
  ownWorkMs,
  stallsIn,
  stallTotal,
} from "./stalls";

// The 244 ms steady-scroll frame of ref-sim run 2
// (bench/results/m1/refsim-txt-2026-10-09-104421.json): the main thread ran
// one 10 ms rAF callback, inside rendering, and nothing else.
const run2: Loaf = {
  startTime: 11368.700000047684,
  duration: 251.39999997615817,
  blockingDuration: 0,
  renderStart: 11608.600000023842,
  scripts: [{ startTime: 11608.700000047684, duration: 10.299999952316284 }],
};

describe("platform frame stalls", () => {
  it("attributes an idle long frame to the platform, minus our work and one frame", () => {
    expect(ownWorkMs(run2)).toBeCloseTo(11.5, 1);
    expect(frameStallMs(run2)).toBeCloseTo(251.4 - 11.5 - FRAME_MS, 1);
    // The rAF interval that ended at this frame: 244.3 ms raw, ~21 ms ours.
    const gapEnd = 11608.6;
    const gapStart = gapEnd - 244.3;
    const own = 244.3 - stallTotal(stallsIn([run2], gapStart, gapEnd));
    expect(own).toBeGreaterThan(FRAME_MS);
    expect(own).toBeLessThan(50);
  });

  it("counts a frame with a blocking task fully as ours", () => {
    expect(frameStallMs({ ...run2, blockingDuration: 12 })).toBe(0);
  });

  it("doesn't count our scripts before rendering as a stall", () => {
    const busy: Loaf = {
      ...run2,
      scripts: [{ startTime: run2.startTime + 5, duration: 230 }],
    };
    expect(frameStallMs(busy)).toBe(0);
    const someWork: Loaf = { ...busy, scripts: [{ startTime: run2.startTime + 5, duration: 20 }] };
    expect(frameStallMs(someWork)).toBeCloseTo(239.9 - 20 - FRAME_MS, 1);
  });

  it("doesn't let rendering work after the wait void the stall", () => {
    // The 404 ms steady-scroll interval of ref-sim run 4
    // (bench/results/m1/refsim-txt-2026-10-13-170045.json): 401 ms idle,
    // then a 16.7 ms rAF callback and rendering, 20 ms in all.
    const run4: Loaf = {
      startTime: 11305,
      duration: 421.1,
      blockingDuration: 0,
      renderStart: 11706,
      scripts: [{ startTime: 11706, duration: 16.7 }],
    };
    expect(ownWorkMs(run4)).toBeCloseTo(20.1, 1);
    expect(frameStallMs(run4)).toBeCloseTo(401 - FRAME_MS, 1);
  });

  it("counts a frame wait with nothing blocking as the platform's, minus one frame", () => {
    // A 223 ms wait for the first frame (txt-2g probe, 2026-10-13).
    expect(frameWaitStallMs([], 1000, 1223)).toBeCloseTo(223 - FRAME_MS, 1);
    expect(frameWaitStallMs([], 1000, 1010)).toBe(0);
  });

  it("counts a frame wait fully as ours if a task blocked in it", () => {
    const blocking: Loaf = { ...run2, startTime: 1100, duration: 80, blockingDuration: 30 };
    expect(frameWaitStallMs([blocking], 1000, 1223)).toBe(0);
    // A blocking frame outside the window doesn't count.
    expect(frameWaitStallMs([blocking], 1300, 1500)).toBeGreaterThan(0);
  });

  it("only counts the part of a stall inside the measured interval", () => {
    const s = stallsIn([run2], run2.startTime, run2.startTime + 100);
    expect(stallTotal(s)).toBeCloseTo(100, 1);
    expect(stallsIn([run2], 0, run2.startTime)).toEqual([]);
  });
});
