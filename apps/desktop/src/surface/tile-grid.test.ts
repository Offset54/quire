import { describe, expect, it } from "vitest";
import { neededTiles, visibleTiles, wantedRange, type GridInput } from "./tile-grid";

const base: GridInput = {
  offset: 0,
  viewportWidth: 1920,
  viewportHeight: 1080,
  extentWidth: 1920,
  extentHeight: 300_000_000,
  tileSize: 512,
  direction: 0,
};

describe("tile grid", () => {
  it("covers the viewport with whole tiles", () => {
    const tiles = visibleTiles({ ...base, offset: 1000 });
    // rows 1..4 (1000..2080 px), 4 columns (1920 / 512)
    expect(new Set(tiles.map((t) => t.row))).toEqual(new Set([1, 2, 3, 4]));
    expect(tiles).toHaveLength(16);
  });

  it("lists visible tiles first, then near, then prefetch in the scroll direction", () => {
    const tiles = neededTiles({ ...base, offset: 100_000, direction: 1 });
    const order = tiles.map((t) => t.priority);
    const firstNear = order.indexOf("near");
    const firstPrefetch = order.indexOf("prefetch");
    expect(order.slice(0, firstNear).every((p) => p === "visible")).toBe(true);
    expect(firstPrefetch).toBeGreaterThan(firstNear);
    const prefetchRows = tiles.filter((t) => t.priority === "prefetch").map((t) => t.row);
    expect((Math.min(...prefetchRows) + 1) * 512).toBeGreaterThan(100_000 + 1080 + 540);
    // No tile appears twice.
    expect(new Set(tiles.map((t) => `${t.col}:${t.row}`)).size).toBe(tiles.length);
  });

  it("stops at the ends of the document", () => {
    const top = neededTiles({ ...base, offset: 0, direction: -1 });
    expect(Math.min(...top.map((t) => t.row))).toBe(0);
    const end = base.extentHeight - 1080;
    const bottom = neededTiles({ ...base, offset: end, direction: 1 });
    expect(Math.max(...bottom.map((t) => t.row))).toBe(Math.ceil(base.extentHeight / 512) - 1);
  });

  it("never requests a tile that the wanted range would drop on arrival", () => {
    for (const direction of [-1, 0, 1] as const) {
      const offset = 5_000_000;
      const [lo, hi] = wantedRange(offset, base.viewportHeight);
      for (const t of neededTiles({ ...base, offset, direction })) {
        expect((t.row + 1) * 512).toBeGreaterThan(lo);
        expect(t.row * 512).toBeLessThan(hi);
      }
    }
  });
});
