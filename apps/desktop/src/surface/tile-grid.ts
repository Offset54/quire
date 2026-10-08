// Which tiles a surface needs, and how urgently (§5.2 priorities).

export type TilePriority = "visible" | "near" | "prefetch";

export interface TileCoord {
  col: number;
  row: number;
}

export interface NeededTile extends TileCoord {
  priority: TilePriority;
}

export interface GridInput {
  /** Scroll offset, device px. */
  offset: number;
  viewportWidth: number;
  viewportHeight: number;
  /** Document extent, device px. */
  extentWidth: number;
  extentHeight: number;
  tileSize: number;
  /** +1 scrolling down, -1 up, 0 idle: prefetch leans that way. */
  direction: -1 | 0 | 1;
}

/** Tile rows fully or partly inside [top, bottom). */
export function rowRange(
  top: number,
  bottom: number,
  tileSize: number,
  maxRow: number,
): [number, number] {
  const first = Math.max(0, Math.floor(top / tileSize));
  const last = Math.min(maxRow, Math.ceil(bottom / tileSize) - 1);
  return [first, last];
}

export function visibleTiles(g: GridInput): TileCoord[] {
  const out: TileCoord[] = [];
  const cols = Math.ceil(Math.min(g.viewportWidth, g.extentWidth) / g.tileSize);
  const maxRow = Math.max(0, Math.ceil(g.extentHeight / g.tileSize) - 1);
  const [r0, r1] = rowRange(g.offset, g.offset + g.viewportHeight, g.tileSize, maxRow);
  for (let row = r0; row <= r1; row++) for (let col = 0; col < cols; col++) out.push({ col, row });
  return out;
}

/** Near band on each side of the viewport, in viewports. */
const NEAR = 0.5;
/** Prefetch band beyond the near band, in the scroll direction, in viewports. */
const PREFETCH = 1;

/**
 * The document range a surface may still want tiles for: everything
 * `neededTiles` can ask for, in either direction. Tiles outside it are
 * dropped on arrival without decoding.
 */
export function wantedRange(offset: number, viewportHeight: number): [number, number] {
  return [
    offset - (NEAR + PREFETCH) * viewportHeight,
    offset + (1 + NEAR + PREFETCH) * viewportHeight,
  ];
}

/**
 * Visible tiles first, then half a viewport above and below ("near"), then
 * one more viewport in the scroll direction ("prefetch").
 */
export function neededTiles(g: GridInput): NeededTile[] {
  const cols = Math.ceil(Math.min(g.viewportWidth, g.extentWidth) / g.tileSize);
  const maxRow = Math.max(0, Math.ceil(g.extentHeight / g.tileSize) - 1);
  const vh = g.viewportHeight;
  const top = g.offset;
  const bottom = g.offset + vh;
  const seen = new Set<number>();
  const out: NeededTile[] = [];
  const add = (from: number, to: number, priority: TilePriority) => {
    const [r0, r1] = rowRange(from, to, g.tileSize, maxRow);
    // Rows nearest the viewport first.
    const rows: number[] = [];
    for (let r = r0; r <= r1; r++) rows.push(r);
    const centre = (top + bottom) / 2 / g.tileSize;
    rows.sort((a, b) => Math.abs(a - centre) - Math.abs(b - centre));
    for (const row of rows) {
      if (seen.has(row)) continue;
      seen.add(row);
      for (let col = 0; col < cols; col++) out.push({ col, row, priority });
    }
  };
  add(top, bottom, "visible");
  add(top - NEAR * vh, bottom + NEAR * vh, "near");
  if (g.direction > 0) add(bottom + NEAR * vh, bottom + (NEAR + PREFETCH) * vh, "prefetch");
  else if (g.direction < 0) add(top - (NEAR + PREFETCH) * vh, top - NEAR * vh, "prefetch");
  return out;
}
