// Light Markdown decoration of the source view (M2 task 8): each row gets a
// class from the kind of the block it is in (from the core's block index,
// `md_blocks`), and inline spans for code, links, emphasis and markers from
// a small tokenizer over the row's text. It styles; it never changes text.

import { invoke } from "@tauri-apps/api/core";
import type { Row } from "../txt/protocol";
import type { RowDeco, TxtView } from "../txt/txt-view";
import { KIND } from "./protocol";

export interface BlockSpan {
  start: number;
  end: number;
  kind: number;
  detail: number;
}

export function decodeBlocks(buf: ArrayBuffer): { revision: number; blocks: BlockSpan[] } {
  const v = new DataView(buf);
  const revision = Number(v.getBigUint64(0, true));
  const blocks: BlockSpan[] = [];
  for (let p = 8; p + 18 <= buf.byteLength; p += 18) {
    blocks.push({
      start: Number(v.getBigUint64(p, true)),
      end: Number(v.getBigUint64(p + 8, true)),
      kind: v.getUint8(p + 16),
      detail: v.getUint8(p + 17),
    });
  }
  return { revision, blocks };
}

/** Row class for a block kind. */
export function blockClass(b: BlockSpan | undefined): string {
  if (!b) return "";
  switch (b.kind) {
    case KIND.heading:
      return `md-src-h md-src-h${Math.min(6, Math.max(1, b.detail))}`;
    case KIND.code:
      return "md-src-code";
    case KIND.frontMatter:
      return "md-src-meta";
    case KIND.html:
      return "md-src-html";
    case KIND.math:
      return "md-src-math";
    case KIND.quote:
      return "md-src-quote";
    case KIND.table:
      return "md-src-table";
    default:
      return "";
  }
}

const INLINE: [RegExp, string][] = [
  // Code spans first: nothing inside them is markup.
  [/(`+)(?!`)[\s\S]*?[^`]\1(?!`)/g, "md-tok-code"],
  [/!?\[[^\]\n]*\]\([^)\n]*\)/g, "md-tok-link"],
  [/!?\[[^\]\n]+\]\[[^\]\n]*\]/g, "md-tok-link"],
  [/<(?:https?|mailto):[^>\s]+>/g, "md-tok-link"],
  [/(\*\*|__)(?=\S)[^\n]*?\S\1/g, "md-tok-strong"],
  [/(?<![\w*])\*(?=[^\s*])[^*\n]*?[^\s*]\*(?![\w*])/g, "md-tok-em"],
  [/(?<![\w_])_(?=[^\s_])[^_\n]*?[^\s_]_(?![\w_])/g, "md-tok-em"],
  [/~~(?=\S)[^\n]*?\S~~/g, "md-tok-del"],
];

const MARKERS: [RegExp, string][] = [
  [/^#{1,6}(?=\s|$)/, "md-tok-mark"],
  [/^\s{0,3}>(?:\s?>)*/, "md-tok-mark"],
  [/^\s*(?:[-*+]|\d{1,9}[.)])(?=\s)(?:\s+\[[ xX]\])?/, "md-tok-mark"],
];

/** Inline spans of a line of Markdown (UTF-16 ranges, sorted, disjoint). */
export function inlineSpans(text: string): [number, number, string][] {
  const out: [number, number, string][] = [];
  const free = (a: number, b: number) => out.every(([x, y]) => b <= x || a >= y);
  for (const [re, cls] of MARKERS) {
    const m = re.exec(text);
    if (m && m[0].trim().length > 0 && free(m.index, m.index + m[0].length)) {
      out.push([m.index, m.index + m[0].length, cls]);
    }
  }
  for (const [re, cls] of INLINE) {
    re.lastIndex = 0;
    for (let m = re.exec(text); m; m = re.exec(text)) {
      const a = m.index;
      const b = a + m[0].length;
      if (b > a && free(a, b)) out.push([a, b, cls]);
      if (m[0].length === 0) re.lastIndex++;
    }
  }
  return out.sort((x, y) => x[0] - y[0]);
}

/** Decorates a source view from the core's block map. */
export class SourceDecorator {
  private blocks: BlockSpan[] = [];
  private covered: [number, number] = [0, -1];
  private timer = 0;
  private revision = -1;
  private destroyed = false;

  constructor(
    private readonly doc: number,
    private readonly view: TxtView,
    /** A new block map arrived. */
    private readonly changed?: () => void,
  ) {
    view.setDecorator((row) => this.decorate(row));
  }

  /** The kind of the block at `offset`, if the map covers it (a hint; the
   *  core checks again). */
  kindAt(offset: number): number | undefined {
    return this.blockAt(offset)?.kind;
  }

  private blockAt(offset: number): BlockSpan | undefined {
    let lo = 0;
    let hi = this.blocks.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const b = this.blocks[mid]!;
      if (offset < b.start) hi = mid - 1;
      else if (offset >= b.end) lo = mid + 1;
      else return b;
    }
    return undefined;
  }

  private decorate(row: Row): RowDeco | undefined {
    const b = this.blockAt(row.start);
    const cls = blockClass(b);
    const plain =
      b !== undefined &&
      (b.kind === KIND.code ||
        b.kind === KIND.frontMatter ||
        b.kind === KIND.html ||
        b.kind === KIND.math);
    const spans = plain || row.text.length > 4096 ? [] : inlineSpans(row.text);
    if (!cls && spans.length === 0) return undefined;
    return { cls, spans };
  }

  /** The view moved or the text changed: refresh the block map soon. */
  update(revision: number): void {
    const [from, to] = this.view.windowRange();
    if (revision === this.revision && from >= this.covered[0] && to <= this.covered[1]) return;
    if (this.timer !== 0) return;
    this.timer = window.setTimeout(() => {
      this.timer = 0;
      void this.fetch();
    }, 30);
  }

  private async fetch(): Promise<void> {
    const [from, to] = this.view.windowRange();
    const lo = Math.max(0, from - 16 * 1024);
    const hi = to + 16 * 1024;
    try {
      const buf = await invoke<ArrayBuffer>("md_blocks", { doc: this.doc, from: lo, to: hi });
      if (this.destroyed) return;
      const m = decodeBlocks(buf);
      this.blocks = m.blocks;
      this.revision = m.revision;
      this.covered = [lo, hi];
      this.view.redecorate();
      this.changed?.();
    } catch {
      // Decoration is optional; the next update tries again.
    }
  }

  destroy(): void {
    this.destroyed = true;
    clearTimeout(this.timer);
    this.view.setDecorator(undefined);
  }
}
