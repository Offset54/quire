import { describe, expect, it } from "vitest";
import { PdfLayout, fitPageScale, fitWidthScale } from "./layout";

function sizes(n: number, w = 600, h = 800): Float32Array {
  const s = new Float32Array(2 * n);
  for (let i = 0; i < n; i++) {
    s[2 * i] = w;
    s[2 * i + 1] = h;
  }
  return s;
}

const base = { mode: "continuous" as const, rotation: 0, scale: 1, gap: 10, viewportWidth: 1000 };

describe("PdfLayout", () => {
  it("stacks pages with gaps and centers them", () => {
    const l = new PdfLayout({ ...base, sizes: sizes(3) });
    expect(l.height).toBe(10 + 3 * 810);
    expect(l.width).toBe(1000);
    expect(l.box(0)).toEqual({ page: 0, x: 200, y: 10, width: 600, height: 800 });
    expect(l.box(2)?.y).toBe(10 + 2 * 810);
  });

  it("finds the pages in a viewport by binary search", () => {
    const l = new PdfLayout({ ...base, sizes: sizes(10_000) });
    const y = l.box(4000)!.y;
    const pages = l.pagesIn(y - 15, y + 900).map((b) => b.page);
    expect(pages).toEqual([3999, 4000, 4001]);
    expect(l.pageAt(y + 400)).toBe(4000);
    expect(l.pagesIn(-100, 5)).toEqual([]);
  });

  it("widens the extent for pages wider than the viewport", () => {
    const l = new PdfLayout({ ...base, sizes: sizes(2), scale: 4 });
    expect(l.width).toBe(2400 + 20);
    expect(l.box(1)?.x).toBe(10);
  });

  it("swaps width and height when rotated", () => {
    const l = new PdfLayout({ ...base, sizes: sizes(1), rotation: 90 });
    expect(l.box(0)).toMatchObject({ width: 800, height: 600 });
  });

  it("lays out two pages per row", () => {
    const s = sizes(3);
    s[3] = 400; // page 1 is shorter
    const l = new PdfLayout({ ...base, sizes: s, mode: "twoUp", viewportWidth: 500 });
    expect(l.rows).toBe(2);
    expect(l.width).toBe(1230);
    const a = l.box(0)!;
    const b = l.box(1)!;
    expect(a.x).toBe(10);
    expect(b.x).toBe(620);
    expect(b.y).toBe(a.y + 200); // centered in the row
    expect(l.box(2)?.y).toBe(820);
  });

  it("shows one page in single mode", () => {
    const l = new PdfLayout({ ...base, sizes: sizes(5), mode: "single", current: 3 });
    expect(l.height).toBe(820);
    expect(l.box(3)?.y).toBe(10);
    expect(l.box(2)).toBeUndefined();
    expect(l.pagesIn(0, 1000).map((b) => b.page)).toEqual([3]);
  });

  it("computes fit scales", () => {
    expect(fitWidthScale(sizes(1), "continuous", 0, 620, 10, 0)).toBe(1);
    expect(fitPageScale(sizes(1), "continuous", 0, 2000, 420, 10, 0)).toBe(0.5);
  });
});
