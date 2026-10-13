// The preview's scroll position as (block, pixels into it): pure helpers,
// kept apart so they can be tested without a DOM (ADR-0030).

export interface Anchor {
  /** Index of the anchor block in the window. */
  index: number;
  /** Pixels from the block's top to the viewport's top (may be negative
   *  before normalization, or past the block's height). */
  px: number;
}

/**
 * Moves the anchor onto the block that holds the viewport's top, given the
 * window's block heights. `atStart`/`atEnd`: the window holds the
 * document's first/last block (the anchor stops there). `viewH`: viewport
 * height (the end clamp keeps the last block's bottom at the viewport's
 * bottom, or the first block at the top for short documents).
 */
export function normalize(
  a: Anchor,
  heights: number[],
  atStart: boolean,
  atEnd: boolean,
  viewH: number,
): Anchor {
  const n = heights.length;
  if (n === 0) return { index: 0, px: 0 };
  let index = Math.min(Math.max(0, a.index), n - 1);
  let px = a.px;
  while (px < 0 && index > 0) {
    index--;
    px += heights[index]!;
  }
  if (px < 0 && atStart) px = 0;
  while (index < n - 1 && px >= heights[index]!) {
    px -= heights[index]!;
    index++;
  }
  if (atEnd) {
    // Distance from the anchor's top to the content's bottom.
    let below = -px;
    for (let i = index; i < n; i++) below += heights[i]!;
    if (below < viewH) {
      // Scroll back so the bottom sits at the viewport's bottom.
      let back = viewH - below;
      while (back > 0) {
        if (px >= back) {
          px -= back;
          back = 0;
        } else {
          back -= px;
          if (index === 0) {
            px = 0;
            break;
          }
          index--;
          px = heights[index]!;
        }
      }
      if (index === 0 && px < 0) px = 0;
    }
  }
  if (atStart && index === 0 && px < 0) px = 0;
  return { index, px };
}

/** Byte offset shown at the viewport's top: interpolated within the block. */
export function topOffset(start: number, end: number, px: number, height: number): number {
  if (height <= 0) return start;
  const f = Math.min(1, Math.max(0, px / height));
  return Math.floor(start + f * (end - start));
}
