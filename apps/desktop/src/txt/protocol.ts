// TXT view messages (crates/text/src/view.rs): `[u32 header length][header
// JSON][rows]`, little-endian. Offsets are u64 byte offsets, read as numbers
// (exact below 2^53, far beyond any file we open).

export interface LineNumber {
  line: number;
  exact: boolean;
}

export interface ViewHeader {
  revision: number;
  len: number;
  anchorIndex: number;
  rows: number;
  lineCount: LineNumber;
  fraction: number;
  selections: [number, number][];
  primary: [number, number];
  selectionCount: number;
  highlights: [number, number][];
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  readOnly: boolean;
}

export const FLAG_FIRST = 1;
export const FLAG_LAST = 2;
export const FLAG_BREAK = 4;

export interface Row {
  start: number;
  end: number;
  /** Offset of `text`'s first byte (differs from `start` in windowed long rows). */
  textStart: number;
  /** Line number shown in the gutter, or -1. */
  line: number;
  flags: number;
  /** Visual column of `textStart`. */
  col0: number;
  /** UTF-16 indices of characters that stand for one raw byte. */
  subst: number[];
  text: string;
}

export interface ViewMessage {
  header: ViewHeader;
  rows: Row[];
}

const decoder = new TextDecoder();

function u64(v: DataView, at: number): number {
  return Number(v.getBigUint64(at, true));
}

export function decodeView(buf: ArrayBuffer): ViewMessage {
  const v = new DataView(buf);
  const hl = v.getUint32(0, true);
  const header = JSON.parse(decoder.decode(new Uint8Array(buf, 4, hl))) as ViewHeader;
  const rows: Row[] = [];
  let at = 4 + hl;
  for (let i = 0; i < header.rows; i++) {
    const start = u64(v, at);
    const end = u64(v, at + 8);
    const textStart = u64(v, at + 16);
    const lineRaw = v.getBigUint64(at + 24, true);
    const flags = v.getUint32(at + 32, true);
    const col0 = v.getUint32(at + 36, true);
    const n = v.getUint32(at + 40, true);
    const subst: number[] = [];
    for (let k = 0; k < n; k++) subst.push(v.getUint32(at + 44 + 4 * k, true));
    const tl = v.getUint32(at + 44 + 4 * n, true);
    const textAt = at + 48 + 4 * n;
    const text = decoder.decode(new Uint8Array(buf, textAt, tl));
    at = textAt + tl;
    rows.push({
      start,
      end,
      textStart,
      line: lineRaw === 0xffffffffffffffffn ? -1 : Number(lineRaw),
      flags,
      col0,
      subst,
      text,
    });
  }
  return { header, rows };
}

/** Bytes a character of the row text stands for in the document. */
function byteLen(cp: number, index: number, substSet: Set<number> | undefined): number {
  if (substSet?.has(index)) return 1;
  return cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
}

/** Document offset of UTF-16 index `u` in the row's text. */
export function offsetAt(row: Row, u: number): number {
  const set = row.subst.length > 0 ? new Set(row.subst) : undefined;
  let off = row.textStart;
  let i = 0;
  const t = row.text;
  while (i < u && i < t.length) {
    const cp = t.codePointAt(i)!;
    const w = cp > 0xffff ? 2 : 1;
    if (i + w > u) break;
    off += byteLen(cp, i, set);
    i += w;
  }
  return off;
}

/** UTF-16 index in the row's text of document offset `off` (clamped). */
export function indexOf(row: Row, off: number): number {
  const set = row.subst.length > 0 ? new Set(row.subst) : undefined;
  let at = row.textStart;
  let i = 0;
  const t = row.text;
  while (i < t.length && at < off) {
    const cp = t.codePointAt(i)!;
    at += byteLen(cp, i, set);
    i += cp > 0xffff ? 2 : 1;
  }
  return i;
}

/** Where the row's text ends in the document. */
export function textEnd(row: Row): number {
  return offsetAt(row, row.text.length);
}
