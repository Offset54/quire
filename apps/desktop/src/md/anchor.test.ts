import { describe, expect, it } from "vitest";
import { normalize, topOffset } from "./anchor";

describe("preview anchor", () => {
  const h = [100, 50, 200, 30];

  it("moves forward and back across blocks", () => {
    expect(normalize({ index: 0, px: 120 }, h, true, false, 100)).toEqual({ index: 1, px: 20 });
    expect(normalize({ index: 2, px: -10 }, h, true, false, 100)).toEqual({ index: 1, px: 40 });
    expect(normalize({ index: 1, px: -200 }, h, true, false, 100)).toEqual({ index: 0, px: 0 });
  });

  it("does not stop at the window's start when more blocks exist before", () => {
    expect(normalize({ index: 0, px: -20 }, h, false, false, 100)).toEqual({ index: 0, px: -20 });
  });

  it("clamps at the document's end", () => {
    // Content is 380 px; a 100 px viewport ends at 280.
    expect(normalize({ index: 3, px: 25 }, h, true, true, 100)).toEqual({ index: 2, px: 130 });
    // A short document stays at the top.
    expect(normalize({ index: 1, px: 10 }, [20, 20], true, true, 100)).toEqual({ index: 0, px: 0 });
  });

  it("interpolates the top offset within a block", () => {
    expect(topOffset(100, 200, 50, 100)).toBe(150);
    expect(topOffset(100, 200, 500, 100)).toBe(200);
    expect(topOffset(100, 200, 5, 0)).toBe(100);
  });
});
