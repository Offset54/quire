import { describe, expect, it } from "vitest";
import { MIN_THUMB_CSS_PX, ScrollModel } from "./scroll-model";

describe("ScrollModel", () => {
  // 10M rows × 30 px: far beyond any browser element height.
  const extent = 10_000_000 * 30;

  it("clamps to the document", () => {
    const m = new ScrollModel(extent, 1080);
    expect(m.scrollTo(-5)).toBe(false);
    expect(m.value).toBe(0);
    m.scrollTo(Number.MAX_SAFE_INTEGER);
    expect(m.value).toBe(extent - 1080);
    expect(m.scrollBy(10)).toBe(false);
  });

  it("addresses every row exactly near the end of a huge document", () => {
    const m = new ScrollModel(extent, 1080);
    m.scrollTo(9_999_000 * 30);
    expect(m.value / 30).toBe(9_999_000);
    m.scrollBy(30);
    expect(m.value / 30).toBe(9_999_001);
  });

  it("keeps a minimum thumb and maps it both ways", () => {
    const m = new ScrollModel(extent, 1080);
    const track = 700;
    expect(m.thumb(track).length).toBe(MIN_THUMB_CSS_PX);
    m.scrollTo(m.max / 2);
    const { top } = m.thumb(track);
    expect(top).toBeCloseTo((track - MIN_THUMB_CSS_PX) / 2, 6);
    expect(m.offsetForThumb(top, track)).toBeCloseTo(m.max / 2, 0);
    expect(m.offsetForThumb(-50, track)).toBe(0);
    expect(m.offsetForThumb(10_000, track)).toBe(m.max);
  });

  it("fills the track when everything fits", () => {
    const m = new ScrollModel(500, 1080);
    expect(m.thumb(300)).toEqual({ top: 0, length: 300 });
    expect(m.max).toBe(0);
  });

  it("re-clamps when the extent shrinks", () => {
    const m = new ScrollModel(10_000, 1000);
    m.scrollTo(9_000);
    m.setExtent(5_000);
    expect(m.value).toBe(4_000);
  });
});
