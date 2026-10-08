// Logical scroll position for document surfaces (ADR-0005).
//
// Browsers cap element heights at a few tens of millions of pixels, so
// surfaces never use native scrolling. The scroll offset is a plain number of
// device pixels from the top of the document (exact up to 2^53), the canvas
// stays fixed to the viewport, and a custom scrollbar maps thumb position to
// offset proportionally.

export interface ThumbGeometry {
  /** Thumb top within the track, in CSS px. */
  top: number;
  /** Thumb length in CSS px. */
  length: number;
}

export const MIN_THUMB_CSS_PX = 24;

export class ScrollModel {
  private offset = 0;

  constructor(
    /** Document height in device px. */
    private extent: number,
    /** Viewport height in device px. */
    private viewport: number,
  ) {}

  get value(): number {
    return this.offset;
  }

  get max(): number {
    return Math.max(0, this.extent - this.viewport);
  }

  setExtent(extent: number): void {
    this.extent = extent;
    this.offset = this.clamp(this.offset);
  }

  setViewport(viewport: number): void {
    this.viewport = viewport;
    this.offset = this.clamp(this.offset);
  }

  private clamp(v: number): number {
    return Math.min(Math.max(0, v), this.max);
  }

  /** Sets the offset; returns whether it changed. */
  scrollTo(offset: number): boolean {
    const next = this.clamp(offset);
    const changed = next !== this.offset;
    this.offset = next;
    return changed;
  }

  scrollBy(delta: number): boolean {
    return this.scrollTo(this.offset + delta);
  }

  /** Thumb position for a track `track` CSS px long. */
  thumb(track: number): ThumbGeometry {
    if (this.extent <= this.viewport) return { top: 0, length: track };
    const length = Math.min(
      track,
      Math.max(MIN_THUMB_CSS_PX, (track * this.viewport) / this.extent),
    );
    const travel = track - length;
    const top = this.max === 0 ? 0 : (this.offset / this.max) * travel;
    return { top, length };
  }

  /** Offset for a thumb whose top is at `thumbTop` CSS px in a `track`-long track. */
  offsetForThumb(thumbTop: number, track: number): number {
    const { length } = this.thumb(track);
    const travel = track - length;
    if (travel <= 0) return 0;
    return this.clamp((Math.min(Math.max(0, thumbTop), travel) / travel) * this.max);
  }
}
