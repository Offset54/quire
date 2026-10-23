// Page layout for the PDF view (M3 tasks 4–5, ADR-0038).
//
// Pages are placed in document space (device px, y down) as rows: one page
// per row (continuous), two (two-up), or only the current page (single).
// Row tops are prefix sums, so finding the pages in a viewport is a binary
// search even for 10,000 pages. Sizes are in points as PDFium reports them
// (after the page's own /Rotate); the view rotation swaps them.

export type LayoutMode = "continuous" | "single" | "twoUp";

export interface PageBox {
  page: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayoutInput {
  /** Width and height in points, two numbers per page. */
  sizes: Float32Array;
  mode: LayoutMode;
  /** Clockwise view rotation: 0, 90, 180 or 270. */
  rotation: number;
  /** Device px per point. */
  scale: number;
  /** Space between and around pages, device px. */
  gap: number;
  /** The viewport's width, device px: narrower content is centered in it. */
  viewportWidth: number;
  /** Single-page mode: the page shown. */
  current?: number;
}

export class PdfLayout {
  readonly count: number;
  /** Pages laid out, in order (all of them, or one in single mode). */
  private readonly shown: number[];
  /** Row tops, one more than rows: rowTop[r]..rowTop[r+1] holds row r. */
  private readonly rowTop: Float64Array;
  private readonly rowOf: Map<number, number> | undefined;
  readonly width: number;
  readonly height: number;
  private readonly perRow: number;

  constructor(readonly input: LayoutInput) {
    const { sizes, mode, gap } = input;
    this.count = sizes.length / 2;
    this.perRow = mode === "twoUp" ? 2 : 1;
    if (mode === "single") {
      const p = Math.min(Math.max(0, input.current ?? 0), Math.max(0, this.count - 1));
      this.shown = this.count > 0 ? [p] : [];
      this.rowOf = new Map([[p, 0]]);
    } else {
      this.shown = [];
      this.rowOf = undefined;
    }
    const rows = Math.ceil(this.rowCountPages() / this.perRow);
    this.rowTop = new Float64Array(rows + 1);
    let widest = 0;
    let y = gap;
    for (let r = 0; r < rows; r++) {
      this.rowTop[r] = y;
      let rowH = 0;
      let rowW = gap * (this.perRow - 1);
      for (const p of this.rowPages(r)) {
        const [w, h] = this.pixels(p);
        rowH = Math.max(rowH, h);
        rowW += w;
      }
      widest = Math.max(widest, rowW);
      y += rowH + gap;
    }
    this.rowTop[rows] = y;
    this.width = Math.max(input.viewportWidth, widest + 2 * gap);
    this.height = rows > 0 ? y : 0;
  }

  private rowCountPages(): number {
    return this.input.mode === "single" ? this.shown.length : this.count;
  }

  /** Pages of row `r`. */
  private rowPages(r: number): number[] {
    if (this.input.mode === "single") return r === 0 ? this.shown : [];
    const first = r * this.perRow;
    const out = [];
    for (let p = first; p < Math.min(this.count, first + this.perRow); p++) out.push(p);
    return out;
  }

  private rowOfPage(page: number): number | undefined {
    if (this.rowOf) return this.rowOf.get(page);
    if (page < 0 || page >= this.count) return undefined;
    return Math.floor(page / this.perRow);
  }

  get rows(): number {
    return this.rowTop.length - 1;
  }

  /** The page's shown size in points (after the view rotation). */
  shownSize(page: number): [number, number] {
    const w = this.input.sizes[2 * page] ?? 0;
    const h = this.input.sizes[2 * page + 1] ?? 0;
    return this.input.rotation % 180 === 0 ? [w, h] : [h, w];
  }

  /** The page's shown size in device px. */
  pixels(page: number): [number, number] {
    const [w, h] = this.shownSize(page);
    return [Math.ceil(w * this.input.scale), Math.ceil(h * this.input.scale)];
  }

  /** Where the page is, or undefined if it isn't laid out (single mode). */
  box(page: number): PageBox | undefined {
    const r = this.rowOfPage(page);
    if (r === undefined) return undefined;
    const pages = this.rowPages(r);
    const { gap } = this.input;
    let rowW = gap * (pages.length - 1);
    for (const p of pages) rowW += this.pixels(p)[0];
    let x = (this.width - rowW) / 2;
    const rowH = this.rowTop[r + 1]! - this.rowTop[r]! - gap;
    for (const p of pages) {
      const [w, h] = this.pixels(p);
      if (p === page) {
        return { page, x, y: this.rowTop[r]! + (rowH - h) / 2, width: w, height: h };
      }
      x += w + gap;
    }
    return undefined;
  }

  /** The row containing document y (the nearest one in a gap). */
  rowAt(y: number): number {
    let lo = 0;
    let hi = this.rows - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.rowTop[mid]! <= y) lo = mid;
      else hi = mid - 1;
    }
    return Math.max(0, lo);
  }

  /** Boxes of the pages intersecting [top, bottom). */
  pagesIn(top: number, bottom: number): PageBox[] {
    if (this.rows === 0) return [];
    const out: PageBox[] = [];
    for (let r = this.rowAt(top); r < this.rows && this.rowTop[r]! < bottom; r++) {
      for (const p of this.rowPages(r)) {
        const b = this.box(p);
        if (b && b.y < bottom && b.y + b.height > top) out.push(b);
      }
    }
    return out;
  }

  /** The page at the middle of the viewport, for "page n of N". */
  pageAt(y: number): number {
    if (this.rows === 0) return 0;
    const pages = this.rowPages(this.rowAt(y));
    return pages[0] ?? 0;
  }
}

/** The scale that fits the widest page (or pair) into the viewport width. */
export function fitWidthScale(
  sizes: Float32Array,
  mode: LayoutMode,
  rotation: number,
  viewportWidth: number,
  gap: number,
  page: number,
): number {
  const [w] = shown(sizes, page, rotation);
  const perRow = mode === "twoUp" ? 2 : 1;
  const avail = viewportWidth - gap * (perRow + 1);
  return Math.max(0.01, avail / (w * perRow));
}

/** The scale that fits one whole page into the viewport. */
export function fitPageScale(
  sizes: Float32Array,
  mode: LayoutMode,
  rotation: number,
  viewportWidth: number,
  viewportHeight: number,
  gap: number,
  page: number,
): number {
  const [, h] = shown(sizes, page, rotation);
  return Math.min(
    fitWidthScale(sizes, mode, rotation, viewportWidth, gap, page),
    Math.max(0.01, (viewportHeight - 2 * gap) / h),
  );
}

function shown(sizes: Float32Array, page: number, rotation: number): [number, number] {
  const w = sizes[2 * page] ?? 612;
  const h = sizes[2 * page + 1] ?? 792;
  return rotation % 180 === 0 ? [w, h] : [h, w];
}
