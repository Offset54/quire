import { describe, expect, it } from "vitest";
import { currentIndex, type OutlineEntry } from "./outline";

describe("currentIndex", () => {
  const e: OutlineEntry[] = [
    [0, 1, "a"],
    [100, 2, "b"],
    [250, 2, "c"],
  ];
  it("finds the section the offset is in", () => {
    expect(currentIndex(e, 0)).toBe(0);
    expect(currentIndex(e, 99)).toBe(0);
    expect(currentIndex(e, 100)).toBe(1);
    expect(currentIndex(e, 1000)).toBe(2);
  });
  it("is -1 before the first heading or without headings", () => {
    expect(currentIndex([[10, 1, "x"]], 5)).toBe(-1);
    expect(currentIndex([], 5)).toBe(-1);
  });
});
