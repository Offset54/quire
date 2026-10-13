// Markdown preview window messages (crates/markdown/src/preview.rs `encode`),
// little-endian. Offsets are u64 byte offsets read as numbers (exact below
// 2^53).

export const MAGIC = 0x3157444d; // "MDW1"

/** Block kinds (`preview::kind_code`). */
export const KIND = {
  other: 0,
  frontMatter: 1,
  paragraph: 2,
  heading: 3,
  thematicBreak: 4,
  code: 5,
  html: 6,
  list: 7,
  quote: 8,
  table: 9,
  footnote: 10,
  math: 11,
  definitions: 12,
} as const;

export interface BlockMeta {
  id: number;
  start: number;
  end: number;
  rev: number;
  lines: number;
  kind: number;
  /** HTML for this revision, when the frontend didn't have it. */
  html: string | undefined;
}

export interface WindowMessage {
  complete: boolean;
  /** Request sequence number; 0 for updates the host pushed. */
  seq: number;
  /** Text revision shown. */
  revision: number;
  len: number;
  frontier: number;
  /** Index of the first block in the document. */
  first: number;
  /** Position of the requested offset's block in `blocks`. */
  anchor: number;
  indexedBlocks: number;
  blocks: BlockMeta[];
}

const decoder = new TextDecoder();

export function decodeWindow(buf: ArrayBuffer): WindowMessage {
  const v = new DataView(buf);
  if (v.getUint32(0, true) !== MAGIC) throw new Error("not a preview window message");
  const u64 = (at: number) => Number(v.getBigUint64(at, true));
  const flags = v.getUint32(4, true);
  const m: WindowMessage = {
    complete: (flags & 1) !== 0,
    seq: u64(8),
    revision: u64(16),
    len: u64(24),
    frontier: u64(32),
    first: v.getUint32(40, true),
    anchor: v.getUint32(44, true),
    indexedBlocks: v.getUint32(48, true),
    blocks: [],
  };
  const count = v.getUint32(52, true);
  let p = 56;
  const bytes = new Uint8Array(buf);
  for (let i = 0; i < count; i++) {
    const id = u64(p);
    const start = u64(p + 8);
    const end = u64(p + 16);
    const rev = u64(p + 24);
    const lines = v.getUint32(p + 32, true);
    const kind = v.getUint8(p + 36);
    const hasHtml = v.getUint8(p + 37) !== 0;
    const n = v.getUint32(p + 40, true);
    p += 44;
    const html = hasHtml ? decoder.decode(bytes.subarray(p, p + n)) : undefined;
    p += n;
    m.blocks.push({ id, start, end, rev, lines, kind, html });
  }
  return m;
}
