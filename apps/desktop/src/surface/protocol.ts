// Tile message format on the Channel transport. Must match
// apps/desktop/src-tauri/src/transport.rs (little-endian, 48-byte header).

import { decodeQoi } from "./qoi";

export const HEADER_LEN = 48;
const MAGIC = 0x314c5451; // "QTL1" little-endian

export interface TileId {
  view: number;
  layout: number;
  row: number;
  col: number;
  size: number;
}

export interface TileHeader extends TileId {
  encoding: "raw" | "qoi";
  width: number;
  height: number;
}

/** u64 → number. View, layout and row values stay far below 2^53. */
function u64(v: DataView, at: number): number {
  return Number(v.getBigUint64(at, true));
}

export function parseHeader(buf: ArrayBuffer): TileHeader {
  if (buf.byteLength < HEADER_LEN) throw new Error(`tile message too short: ${buf.byteLength}`);
  const v = new DataView(buf, 0, HEADER_LEN);
  if (v.getUint32(0, true) !== MAGIC) throw new Error("bad tile message magic");
  const encoding = v.getUint32(4, true);
  if (encoding > 1) throw new Error(`unknown tile encoding ${encoding}`);
  return {
    encoding: encoding === 0 ? "raw" : "qoi",
    view: u64(v, 8),
    layout: u64(v, 16),
    row: u64(v, 24),
    col: v.getUint32(32, true),
    size: v.getUint32(36, true),
    width: v.getUint32(40, true),
    height: v.getUint32(44, true),
  };
}

/** RGBA pixels of a channel message. */
export function decodePixels(buf: ArrayBuffer, h: TileHeader): Uint8ClampedArray<ArrayBuffer> {
  const payload = new Uint8Array(buf, HEADER_LEN);
  const rgba = h.encoding === "raw" ? payload : decodeQoi(payload).data;
  const expected = h.width * h.height * 4;
  if (rgba.byteLength < expected) throw new Error("tile payload too short");
  return new Uint8ClampedArray(rgba.buffer as ArrayBuffer, rgba.byteOffset, expected);
}

export function tileKey(t: TileId): string {
  return `${t.view}:${t.layout}:${t.size}:${t.col}:${t.row}`;
}
