// TXT editor surface (§5.4: TXT views render visible lines as virtualized
// DOM text; ADR-0023). Imperative, outside React.
//
// - The core owns the document, the layout (wrapping) and the selections.
//   This view holds a window of visual rows around the viewport, fetched as
//   binary messages, and a fractional position `top` (in rows) within it.
// - Scrolling moves `top` locally and fetches a new window, anchored at the
//   current top row's byte offset, when it nears an edge. Byte anchors keep
//   the viewport still when exact line numbers arrive (§6.1).
// - Edits are sent one at a time, in order; each answer is a fresh window
//   anchored at the caret, so the caret keeps its row on screen.
// - Text input goes through a hidden textarea at the caret, so IME
//   composition and its candidate window work; composition text is shown
//   inline in the caret's row until it's committed.

import { invoke } from "@tauri-apps/api/core";
import {
  FLAG_BREAK,
  FLAG_FIRST,
  FLAG_LAST,
  decodeView,
  indexOf,
  offsetAt,
  type Row,
  type ViewHeader,
  type ViewMessage,
} from "./protocol";

export type MoveTo =
  | "left"
  | "right"
  | "wordLeft"
  | "wordRight"
  | "home"
  | "end"
  | "up"
  | "down"
  | "pageUp"
  | "pageDown"
  | "docStart"
  | "docEnd";

export type Action =
  | { type: "move"; to: MoveTo; extend: boolean }
  | { type: "type"; text: string }
  | { type: "paste"; text: string }
  | { type: "newline" }
  | { type: "tab" }
  | { type: "backspace" }
  | { type: "delete" }
  | { type: "deleteWordLeft" }
  | { type: "deleteWordRight" }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "selectAll" }
  | { type: "selectLine" }
  | { type: "addCursorAbove" }
  | { type: "addCursorBelow" }
  | { type: "addNextOccurrence" }
  | { type: "collapse" }
  | {
      type: "click";
      offset: number;
      mode: "set" | "add" | "extend";
      unit: "char" | "word" | "line";
    }
  | { type: "setSelections"; ranges: [number, number][] }
  | { type: "cut" };

export interface ViewRequest {
  anchor: number;
  caret: boolean;
  before: number;
  after: number;
  layout: { wrap: number | null; tab: number };
  hcols: [number, number] | null;
}

export interface CaretInfo {
  /** 1-based line, if known exactly. */
  line: number | undefined;
  /** 1-based column (characters). */
  col: number;
  offset: number;
  selections: number;
  selectedBytes: number;
}

export interface TxtCallbacks {
  onHeader(h: ViewHeader): void;
  onCaret(c: CaretInfo): void;
  onError(message: string): void;
  /** Keyboard commands the chrome handles (find, save, ...). */
  onCommand(name: string): void;
}

/** Latency of one input, for the HUD and benchmarks. */
export interface InputSample {
  kind: string;
  /** Event time to the frame that shows the result, ms. */
  latencyMs: number;
  /** Time in the core command (round trip), ms. */
  roundTripMs: number;
}

const TAB = 4;

/** Code points in the first `upTo` UTF-16 units of `s`. */
function codePoints(s: string, upTo: number): number {
  let n = 0;
  for (let i = 0; i < upTo; i++) {
    const c = s.charCodeAt(i);
    if (c < 0xdc00 || c > 0xdfff) n++;
  }
  return n;
}
const OVERSCAN_VIEWPORTS = 2;
const LONG_ROW_BYTES = 8 * 1024;

interface RowEl {
  root: HTMLDivElement;
  ln: HTMLSpanElement;
  inner: HTMLDivElement;
  overlay: HTMLDivElement;
  tx: HTMLSpanElement;
  key: string;
  decoKey: string;
}

export class TxtView {
  readonly element: HTMLDivElement;
  private readonly viewport: HTMLDivElement;
  private readonly layer: HTMLDivElement;
  private readonly input: HTMLTextAreaElement;
  private readonly track: HTMLDivElement;
  private readonly thumb: HTMLDivElement;
  private readonly hbar: HTMLDivElement;
  private readonly hthumb: HTMLDivElement;
  private readonly pool: RowEl[] = [];
  private readonly sampleListeners = new Set<(s: InputSample) => void>();

  private rows: Row[] = [];
  private header: ViewHeader | undefined;
  private top = 0;
  private scrollX = 0;
  private maxCols = 0;
  private lineH = 20;
  private cellW = 8;
  private gutterW = 48;
  private viewW = 0;
  private viewH = 0;
  wrap = false;
  private fetching = false;
  private refetch = false;
  private dirty = true;
  private frame = 0;
  private queue: { action: Action; at: number; kind: string }[] = [];
  private sending = false;
  private composing = false;
  private composeText = "";
  private dragAnchor: number | undefined;
  private destroyed = false;
  private resizeObserver: ResizeObserver;
  /** Matches to highlight are part of each view message. */

  constructor(
    host: HTMLElement,
    readonly doc: number,
    private readonly cb: TxtCallbacks,
  ) {
    this.element = document.createElement("div");
    this.element.className = "txt";
    this.viewport = document.createElement("div");
    this.viewport.className = "txt-viewport";
    this.viewport.setAttribute("role", "textbox");
    this.viewport.setAttribute("aria-multiline", "true");
    this.layer = document.createElement("div");
    this.layer.className = "txt-layer";
    this.viewport.append(this.layer);
    this.input = document.createElement("textarea");
    this.input.className = "txt-input";
    this.input.setAttribute("autocomplete", "off");
    this.input.setAttribute("autocorrect", "off");
    this.input.setAttribute("autocapitalize", "off");
    this.input.spellcheck = false;
    this.input.setAttribute("aria-label", "Text editor");
    this.track = document.createElement("div");
    this.track.className = "surface-scrollbar";
    this.thumb = document.createElement("div");
    this.thumb.className = "surface-thumb";
    this.track.append(this.thumb);
    this.hbar = document.createElement("div");
    this.hbar.className = "txt-hbar";
    this.hthumb = document.createElement("div");
    this.hthumb.className = "txt-hthumb";
    this.hbar.append(this.hthumb);
    this.element.append(this.viewport, this.input, this.track, this.hbar);
    host.append(this.element);

    this.measureFont();
    this.installInput();
    this.installPointer();
    this.installScroll();
    this.resizeObserver = new ResizeObserver(() => {
      this.resize();
    });
    this.resizeObserver.observe(this.element);
    this.resize();
  }

  // ---- public API ----------------------------------------------------------

  focus(): void {
    this.input.focus({ preventScroll: true });
  }

  get currentHeader(): ViewHeader | undefined {
    return this.header;
  }

  get visibleRows(): number {
    return Math.max(1, Math.floor(this.viewH / this.lineH));
  }

  /** Rows currently on screen (for tests and benchmarks). */
  visibleText(): string[] {
    const first = Math.floor(this.top);
    return this.rows.slice(first, first + this.visibleRows).map((r) => r.text);
  }

  onInputSample(l: (s: InputSample) => void): () => void {
    this.sampleListeners.add(l);
    return () => this.sampleListeners.delete(l);
  }

  setWrap(wrap: boolean): void {
    if (wrap === this.wrap) return;
    this.wrap = wrap;
    this.scrollX = 0;
    void this.fetchAt(this.topOffset(), this.top - Math.floor(this.top));
  }

  /** Reloads the window (after changes made elsewhere: save, replace-all). */
  refresh(): void {
    void this.fetchAt(this.topOffset(), this.top - Math.floor(this.top));
  }

  /** Scrolls so `offset`'s row is about a third down the viewport. */
  async reveal(offset: number): Promise<void> {
    const rowsAbove = Math.floor(this.visibleRows / 3);
    await this.fetch({ anchor: offset, caret: false }, rowsAbove);
  }

  /** Jumps to a scrollbar fraction 0..1. */
  async seek(fraction: number): Promise<void> {
    const anchor = await invoke<number>("txt_seek", { doc: this.doc, fraction });
    await this.fetch({ anchor, caret: false }, 0);
  }

  /** Sends an action; resolves when it has been applied and drawn. */
  send(action: Action, kind: string = action.type): void {
    const last = this.queue[this.queue.length - 1];
    // Coalesce queued typing: fewer round trips, same result.
    if (last && action.type === "type" && last.action.type === "type") {
      last.action = { type: "type", text: last.action.text + action.text };
    } else {
      this.queue.push({ action, at: performance.now(), kind });
    }
    void this.pump();
  }

  /** Pastes text (sent as a raw request body). */
  paste(text: string): void {
    this.queue.push({ action: { type: "paste", text }, at: performance.now(), kind: "paste" });
    void this.pump();
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.frame);
    this.resizeObserver.disconnect();
    this.element.remove();
  }

  // ---- fetching ------------------------------------------------------------

  private layout(): { wrap: number | null; tab: number } {
    const textW = this.viewW - this.gutterW - 8;
    return {
      wrap: this.wrap ? Math.max(8, Math.floor(textW / this.cellW) - 1) : null,
      tab: TAB,
    };
  }

  private request(anchor: number, caret: boolean, before: number): ViewRequest {
    const v = this.visibleRows;
    const c0 = Math.floor(this.scrollX / this.cellW);
    return {
      anchor,
      caret,
      before,
      after: v * (OVERSCAN_VIEWPORTS + 1),
      layout: this.layout(),
      hcols: [c0, c0 + Math.ceil(this.viewW / this.cellW)],
    };
  }

  private topOffset(): number {
    return this.rows[Math.floor(this.top)]?.start ?? 0;
  }

  private fetchAt(anchor: number, frac: number): Promise<void> {
    return this.fetch({ anchor, caret: false }, this.visibleRows * OVERSCAN_VIEWPORTS, frac);
  }

  /**
   * Fetches a window around `anchor` and places the anchor row `screenRow`
   * rows below the top of the viewport (plus `frac`).
   */
  private async fetch(
    a: { anchor: number; caret: boolean },
    screenRow: number,
    frac = 0,
  ): Promise<void> {
    if (this.fetching) {
      this.refetch = true;
      return;
    }
    this.fetching = true;
    try {
      const before = Math.max(screenRow, this.visibleRows * OVERSCAN_VIEWPORTS);
      const buf = await invoke<ArrayBuffer>("txt_view", {
        doc: this.doc,
        req: this.request(a.anchor, a.caret, before),
      });
      this.adopt(decodeView(buf), screenRow, frac);
    } catch (err) {
      this.cb.onError(String(err));
    } finally {
      this.fetching = false;
    }
    if (this.refetch) {
      this.refetch = false;
      void this.fetchAt(this.topOffset(), this.top - Math.floor(this.top));
    }
  }

  private adopt(m: ViewMessage, screenRow: number, frac: number): void {
    this.rows = m.rows;
    this.header = m.header;
    this.top = m.header.anchorIndex - screenRow + frac;
    this.clampTop();
    for (const r of m.rows) {
      const cols = r.end - r.start > LONG_ROW_BYTES ? r.end - r.start : r.col0 + r.text.length;
      if (cols > this.maxCols) this.maxCols = cols;
    }
    this.cb.onHeader(m.header);
    this.reportCaret();
    this.invalidate();
  }

  private atDocStart(): boolean {
    const r = this.rows[0];
    return !r || r.start === 0;
  }

  private atDocEnd(): boolean {
    const r = this.rows[this.rows.length - 1];
    return (
      !r || (r.flags & FLAG_LAST && !(r.flags & FLAG_BREAK) && r.end === this.header?.len) === true
    );
  }

  private clampTop(): void {
    const maxTop = Math.max(0, this.rows.length - this.visibleRows);
    if (this.top < 0) this.top = this.atDocStart() ? 0 : Math.max(this.top, -0.999);
    if (this.top > maxTop && this.atDocEnd()) this.top = maxTop;
    this.top = Math.max(
      this.atDocStart() ? 0 : -0.999,
      Math.min(this.top, Math.max(0, this.rows.length - 1)),
    );
  }

  /** Fetches more rows if the viewport nears either end of the window. */
  private maybeFetch(): void {
    const v = this.visibleRows;
    const nearTop = this.top < v && !this.atDocStart();
    const nearBottom = this.top + 2 * v > this.rows.length && !this.atDocEnd();
    if (nearTop || nearBottom) {
      const i = Math.max(0, Math.floor(this.top));
      void this.fetchAt(this.rows[i]?.start ?? 0, this.top - i);
    }
  }

  scrollByRows(n: number): void {
    if (this.rows.length === 0) return;
    this.top += n;
    this.clampTop();
    this.maybeFetch();
    this.invalidate();
  }

  // ---- edits ---------------------------------------------------------------

  /** Screen row of the primary caret, if it's in the window. */
  private caretScreenRow(): number | undefined {
    const h = this.header?.primary[1];
    if (h === undefined) return undefined;
    const i = this.rowIndexOf(h);
    if (i === undefined) return undefined;
    const s = i - Math.floor(this.top);
    return s >= 0 && s < this.visibleRows ? s : undefined;
  }

  private rowIndexOf(offset: number): number | undefined {
    // The row holding `offset`; a wrap boundary belongs to the later row.
    let lo = 0;
    let hi = this.rows.length - 1;
    if (hi < 0) return undefined;
    if (offset < this.rows[0]!.start) return undefined;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.rows[mid]!.start <= offset) lo = mid;
      else hi = mid - 1;
    }
    const r = this.rows[lo]!;
    const end = r.flags & FLAG_LAST ? r.end : r.end - 1;
    return offset <= end || lo === this.rows.length - 1 ? lo : undefined;
  }

  private async pump(): Promise<void> {
    if (this.sending) return;
    this.sending = true;
    try {
      while (this.queue.length > 0 && !this.destroyed) {
        const item = this.queue.shift()!;
        // Keep the caret on its screen row; bring it to a third of the
        // viewport if it was off screen.
        const screen = this.caretScreenRow() ?? Math.floor(this.visibleRows / 3);
        const frac = this.caretScreenRow() !== undefined ? this.top - Math.floor(this.top) : 0;
        const req = this.request(0, true, Math.max(screen, this.visibleRows * OVERSCAN_VIEWPORTS));
        const sent = performance.now();
        let buf: ArrayBuffer;
        try {
          if (item.action.type === "paste") {
            buf = await invoke<ArrayBuffer>(
              "txt_paste",
              new TextEncoder().encode(item.action.text),
              {
                headers: { "x-quire-doc": String(this.doc), "x-quire-view": JSON.stringify(req) },
              },
            );
          } else {
            buf = await invoke<ArrayBuffer>("txt_edit", {
              doc: this.doc,
              action: item.action,
              req,
              page: { rows: Math.max(1, this.visibleRows - 1) },
            });
          }
        } catch (err) {
          this.cb.onError(String(err));
          continue;
        }
        const roundTripMs = performance.now() - sent;
        this.adopt(decodeView(buf), screen, frac);
        this.revealCaretX();
        const at = item.at;
        const kind = item.kind;
        requestAnimationFrame(() => {
          // The frame that draws the result has been produced.
          const latencyMs = performance.now() - at;
          for (const l of this.sampleListeners) l({ kind, latencyMs, roundTripMs });
        });
      }
    } finally {
      this.sending = false;
    }
  }

  /** Copies the selection to the clipboard (through the core). */
  async copy(): Promise<void> {
    try {
      await invoke("txt_copy", { doc: this.doc });
    } catch (err) {
      this.cb.onError(String(err));
    }
  }

  // ---- geometry ------------------------------------------------------------

  private measureFont(): void {
    const probe = document.createElement("div");
    probe.className = "txt-row txt-probe";
    const span = document.createElement("span");
    span.className = "txt-text";
    span.textContent = "0".repeat(100);
    probe.append(span);
    this.layer.append(probe);
    const rect = span.getBoundingClientRect();
    this.cellW = rect.width / 100 || 8;
    this.lineH = Math.ceil(probe.getBoundingClientRect().height) || 20;
    probe.remove();
  }

  private resize(): void {
    const r = this.viewport.getBoundingClientRect();
    this.viewW = r.width;
    this.viewH = r.height;
    const digits = String(Math.max(10, this.header?.lineCount.line ?? 10)).length;
    this.gutterW = Math.ceil((digits + 2) * this.cellW);
    if (this.rows.length > 0) this.refresh();
    else void this.fetch({ anchor: 0, caret: false }, 0);
  }

  private invalidate(): void {
    this.dirty = true;
    if (this.frame === 0 && !this.destroyed) {
      this.frame = requestAnimationFrame(() => {
        this.frame = 0;
        this.draw();
      });
    }
  }

  private rowEl(i: number): RowEl {
    let el = this.pool[i];
    if (!el) {
      const root = document.createElement("div");
      root.className = "txt-row";
      const ln = document.createElement("span");
      ln.className = "txt-ln";
      const clip = document.createElement("div");
      clip.className = "txt-clip";
      const inner = document.createElement("div");
      inner.className = "txt-inner";
      const overlay = document.createElement("div");
      overlay.className = "txt-deco";
      const tx = document.createElement("span");
      tx.className = "txt-text";
      inner.append(overlay, tx);
      clip.append(inner);
      root.append(ln, clip);
      this.layer.append(root);
      el = { root, ln, inner, overlay, tx, key: "", decoKey: "" };
      this.pool[i] = el;
    }
    return el;
  }

  private draw(): void {
    if (!this.dirty || !this.header) return;
    this.dirty = false;
    const digits = String(Math.max(10, this.header.lineCount.line)).length;
    const gutterW = Math.ceil((digits + 2) * this.cellW);
    if (gutterW !== this.gutterW) this.gutterW = gutterW;
    this.element.style.setProperty("--gutter", `${this.gutterW}px`);
    this.element.style.setProperty("--line-h", `${this.lineH}px`);
    const first = Math.floor(this.top);
    const frac = this.top - first;
    const n = this.visibleRows + 2;
    this.layer.style.transform = `translateY(${-frac * this.lineH}px)`;
    const sels = this.header.selections;
    const hls = this.header.highlights;
    const primary = this.header.primary;
    for (let k = 0; k < n; k++) {
      const el = this.rowEl(k);
      const row = this.rows[first + k];
      if (!row) {
        el.root.style.display = "none";
        el.key = "";
        continue;
      }
      el.root.style.display = "";
      el.root.style.transform = `translateY(${k * this.lineH}px)`;
      el.root.dataset.index = String(first + k);
      const key = `${row.textStart}:${row.end}:${this.header.revision}`;
      if (el.key !== key) {
        el.key = key;
        el.tx.textContent = row.text;
        el.ln.textContent = row.flags & FLAG_FIRST && row.line >= 0 ? String(row.line + 1) : "";
        el.decoKey = "";
      }
      el.inner.style.transform = `translateX(${row.col0 * this.cellW - this.scrollX}px)`;
      const decoKey = `${key}:${JSON.stringify(sels)}:${hls.length}:${hls[0]?.[0] ?? -1}:${this.composing ? this.composeText : ""}`;
      if (el.decoKey !== decoKey) {
        el.decoKey = decoKey;
        this.decorate(el, row, sels, hls, primary);
      }
    }
    this.updateScrollbars();
    this.placeInput();
  }

  /** Draws selections, highlights and carets of one row (row-relative). */
  private decorate(
    el: RowEl,
    row: Row,
    sels: [number, number][],
    hls: [number, number][],
    primary: [number, number],
  ): void {
    el.overlay.replaceChildren();
    const node = el.tx.firstChild;
    const textEndOff = row.end;
    const lastOfLine = (row.flags & FLAG_LAST) !== 0;
    const rangeX = (u: number): number => {
      if (!node || u <= 0) return 0;
      const len = node.textContent?.length ?? 0;
      const r = document.createRange();
      r.setStart(node, Math.min(u, len));
      r.setEnd(node, Math.min(u, len));
      const base = el.tx.getBoundingClientRect().left;
      const rect = r.getClientRects()[0] ?? r.getBoundingClientRect();
      return rect.left - base;
    };
    const span = (a: number, b: number, cls: string, extendBreak: boolean) => {
      const from = Math.max(a, row.textStart);
      const to = Math.min(b, textEndOff);
      if (to < from || (to === from && !extendBreak)) return;
      const x0 = rangeX(indexOf(row, from));
      const x1 = rangeX(indexOf(row, to)) + (extendBreak ? this.cellW * 0.6 : 0);
      if (x1 <= x0) return;
      const d = document.createElement("div");
      d.className = cls;
      d.style.left = `${x0}px`;
      d.style.width = `${x1 - x0}px`;
      el.overlay.append(d);
    };
    for (const [a, b] of hls) {
      if (b > row.start && a < row.end) span(a, b, "txt-hl", false);
    }
    for (const [anchor, head] of sels) {
      const a = Math.min(anchor, head);
      const b = Math.max(anchor, head);
      if (a !== b && b >= row.start && a <= row.end) {
        const coversBreak =
          lastOfLine && (row.flags & FLAG_BREAK) !== 0 && b > row.end && a <= row.end;
        span(a, b, "txt-sel", coversBreak);
      }
      // The caret belongs to this row unless it sits at a wrap boundary
      // (then it's at the start of the next row).
      const inRow = head >= row.start && (head < row.end || (head === row.end && lastOfLine));
      if (inRow && head >= row.textStart && head <= textEndOff) {
        const c = document.createElement("div");
        c.className =
          head === primary[1] && anchor === primary[0] ? "txt-caret primary" : "txt-caret";
        c.style.left = `${rangeX(indexOf(row, head))}px`;
        el.overlay.append(c);
      }
    }
  }

  private updateScrollbars(): void {
    const h = this.header;
    if (!h) return;
    const track = this.track.clientHeight;
    const lines = Math.max(1, h.lineCount.line);
    const visible = this.visibleRows;
    const length = Math.min(track, Math.max(24, (track * visible) / lines));
    const anchorRow = h.anchorIndex;
    const f = Math.min(1, Math.max(0, h.fraction + (this.top - anchorRow) / lines));
    this.thumb.style.height = `${length}px`;
    this.thumb.style.transform = `translateY(${f * (track - length)}px)`;
    this.thumb.style.display =
      lines <= visible && this.atDocStart() && this.atDocEnd() ? "none" : "";
    // Horizontal (no wrap).
    const contentW = this.maxCols * this.cellW;
    const viewW = this.viewW - this.gutterW;
    const show = !this.wrap && contentW > viewW;
    this.hbar.style.display = show ? "" : "none";
    if (show) {
      const tw = this.hbar.clientWidth;
      const len = Math.max(24, (tw * viewW) / contentW);
      this.hthumb.style.width = `${len}px`;
      this.hthumb.style.transform = `translateX(${(this.scrollX / Math.max(1, contentW - viewW)) * (tw - len)}px)`;
    }
  }

  /** Puts the hidden textarea at the primary caret (IME candidate window). */
  private placeInput(): void {
    const caret = this.layer.querySelector<HTMLDivElement>(".txt-caret.primary");
    const vr = this.element.getBoundingClientRect();
    if (caret) {
      const r = caret.getBoundingClientRect();
      this.input.style.transform = `translate(${r.left - vr.left}px, ${r.top - vr.top}px)`;
    }
    this.input.style.height = `${this.lineH}px`;
  }

  private reportCaret(): void {
    const h = this.header;
    if (!h) return;
    const head = h.primary[1];
    const i = this.rowIndexOf(head);
    let line: number | undefined;
    let col = 1;
    if (i !== undefined) {
      // Walk back to the line's first row.
      let k = i;
      let chars = 0;
      while (k >= 0) {
        const r = this.rows[k]!;
        const upTo = k === i ? indexOf(r, head) : r.text.length;
        chars += codePoints(r.text, upTo);
        if (r.flags & FLAG_FIRST) {
          if (r.line >= 0) line = r.line + 1;
          break;
        }
        k--;
      }
      col = chars + 1;
    }
    const sel = h.selections.reduce((s, [a, b]) => s + Math.abs(b - a), 0);
    this.cb.onCaret({ line, col, offset: head, selections: h.selectionCount, selectedBytes: sel });
  }

  private revealCaretX(): void {
    if (this.wrap) return;
    const caret = this.layer.querySelector<HTMLDivElement>(".txt-caret.primary");
    if (!caret) {
      this.invalidate();
      return;
    }
    // Drawn on the next frame; check after it.
    requestAnimationFrame(() => {
      const c = this.layer.querySelector<HTMLDivElement>(".txt-caret.primary");
      if (!c) return;
      const vr = this.viewport.getBoundingClientRect();
      const x = c.getBoundingClientRect().left - vr.left - this.gutterW;
      const viewW = this.viewW - this.gutterW;
      if (x < 0 || x > viewW - this.cellW * 2) {
        this.scrollX = Math.max(0, this.scrollX + x - viewW / 2);
        this.invalidateAll();
      }
    });
  }

  private invalidateAll(): void {
    for (const el of this.pool) el.decoKey = "";
    this.invalidate();
  }

  // ---- input ---------------------------------------------------------------

  private installInput(): void {
    const input = this.input;
    this.viewport.addEventListener("mousedown", () => {
      setTimeout(() => {
        this.focus();
      }, 0);
    });

    input.addEventListener("compositionstart", () => {
      this.composing = true;
      this.composeText = "";
    });
    input.addEventListener("compositionupdate", (e) => {
      this.composeText = e.data;
      this.showComposition();
    });
    input.addEventListener("compositionend", (e) => {
      this.composing = false;
      this.composeText = "";
      input.value = "";
      if (e.data) this.send({ type: "type", text: e.data }, "ime");
      this.invalidateAll();
    });
    input.addEventListener("beforeinput", (e) => {
      if (e.isComposing || this.composing || e.inputType === "insertCompositionText") return;
      e.preventDefault();
      switch (e.inputType) {
        case "insertText":
          if (e.data) this.send({ type: "type", text: e.data });
          break;
        case "insertLineBreak":
        case "insertParagraph":
          this.send({ type: "newline" });
          break;
        case "deleteContentBackward":
          this.send({ type: "backspace" });
          break;
        case "deleteContentForward":
          this.send({ type: "delete" });
          break;
        case "deleteWordBackward":
          this.send({ type: "deleteWordLeft" });
          break;
        case "deleteWordForward":
          this.send({ type: "deleteWordRight" });
          break;
        case "historyUndo":
          this.send({ type: "undo" });
          break;
        case "historyRedo":
          this.send({ type: "redo" });
          break;
        default:
          break;
      }
    });
    input.addEventListener("input", () => {
      // Anything that slipped through (e.g. after composition) is dropped:
      // the core is the source of truth.
      if (!this.composing) input.value = "";
    });
    input.addEventListener("copy", (e) => {
      e.preventDefault();
      void this.copy();
    });
    input.addEventListener("cut", (e) => {
      e.preventDefault();
      this.send({ type: "cut" });
    });
    input.addEventListener("paste", (e) => {
      e.preventDefault();
      const text = e.clipboardData?.getData("text/plain") ?? "";
      if (text) this.paste(text);
    });
    input.addEventListener("keydown", (e) => {
      if (this.onKey(e)) e.stopPropagation();
    });
    input.addEventListener("focus", () => {
      this.element.classList.add("focused");
    });
    input.addEventListener("blur", () => {
      this.element.classList.remove("focused");
    });
  }

  private onKey(e: KeyboardEvent): boolean {
    if (e.isComposing || this.composing) return false;
    const ctrl = e.ctrlKey || e.metaKey;
    const shift = e.shiftKey;
    const move = (to: MoveTo): true => {
      e.preventDefault();
      this.send({ type: "move", to, extend: shift });
      return true;
    };
    const act = (a: Action): true => {
      e.preventDefault();
      this.send(a);
      return true;
    };
    const command = (name: string): true => {
      e.preventDefault();
      this.cb.onCommand(name);
      return true;
    };
    switch (e.key) {
      case "ArrowLeft":
        return move(ctrl ? "wordLeft" : "left");
      case "ArrowRight":
        return move(ctrl ? "wordRight" : "right");
      case "ArrowUp":
        if (ctrl && e.altKey) return act({ type: "addCursorAbove" });
        if (ctrl) {
          e.preventDefault();
          this.scrollByRows(-1);
          return true;
        }
        return move("up");
      case "ArrowDown":
        if (ctrl && e.altKey) return act({ type: "addCursorBelow" });
        if (ctrl) {
          e.preventDefault();
          this.scrollByRows(1);
          return true;
        }
        return move("down");
      case "Home":
        return move(ctrl ? "docStart" : "home");
      case "End":
        return move(ctrl ? "docEnd" : "end");
      case "PageUp":
        return move("pageUp");
      case "PageDown":
        return move("pageDown");
      case "Tab":
        if (!ctrl) return act({ type: "tab" });
        return false;
      case "Escape":
        return act({ type: "collapse" });
      case "Enter":
        return act({ type: "newline" });
      case "Backspace":
        return act(ctrl ? { type: "deleteWordLeft" } : { type: "backspace" });
      case "Delete":
        return act(ctrl ? { type: "deleteWordRight" } : { type: "delete" });
      case "F3":
        return command(shift ? "findPrevious" : "findNext");
      default:
        break;
    }
    if (e.altKey && !ctrl && e.key.toLowerCase() === "z") return command("toggleWrap");
    if (!ctrl) return false;
    switch (e.key.toLowerCase()) {
      case "a":
        return act({ type: "selectAll" });
      case "z":
        return act(shift ? { type: "redo" } : { type: "undo" });
      case "y":
        return act({ type: "redo" });
      case "d":
        return act({ type: "addNextOccurrence" });
      case "l":
        return act({ type: "selectLine" });
      case "f":
        return command("find");
      case "h":
        return command("replace");
      case "g":
        return command("gotoLine");
      case "s":
        return command(shift ? "saveAs" : "save");
      case "o":
        return command("open");
      case "n":
        return command("new");
      default:
        return false;
    }
  }

  /** Shows the IME composition inline in the caret's row. */
  private showComposition(): void {
    const h = this.header?.primary[1];
    if (h === undefined) return;
    const i = this.rowIndexOf(h);
    if (i === undefined) return;
    const k = i - Math.floor(this.top);
    const el = this.pool[k];
    const row = this.rows[i];
    if (!el || !row) return;
    const u = indexOf(row, h);
    const before = document.createTextNode(row.text.slice(0, u));
    const comp = document.createElement("span");
    comp.className = "txt-compose";
    comp.textContent = this.composeText;
    const after = document.createTextNode(row.text.slice(u));
    el.tx.replaceChildren(before, comp, after);
    el.key = ""; // redraw plain text after the composition ends
    el.overlay.replaceChildren();
    const vr = this.element.getBoundingClientRect();
    const r = comp.getBoundingClientRect();
    this.input.style.transform = `translate(${r.right - vr.left}px, ${r.top - vr.top}px)`;
  }

  // ---- pointer ---------------------------------------------------------------

  /** Document offset under a client point, if over a row. */
  private hit(x: number, y: number): number | undefined {
    const vr = this.viewport.getBoundingClientRect();
    const rowK = Math.floor((y - vr.top) / this.lineH + (this.top - Math.floor(this.top)));
    const i = Math.floor(this.top) + Math.max(0, rowK);
    const row = this.rows[Math.min(i, this.rows.length - 1)];
    if (!row) return undefined;
    const el = this.pool[i - Math.floor(this.top)];
    const node = el?.tx.firstChild;
    if (!el || !node || x - vr.left < this.gutterW) {
      return x - vr.left < this.gutterW ? row.start : row.textStart;
    }
    // Clamp the point vertically into the row so the browser finds its text.
    const tr = el.tx.getBoundingClientRect();
    const py = tr.top + tr.height / 2;
    let u: number | undefined;
    const pos = document.caretPositionFromPoint(x, py);
    if (pos?.offsetNode === node) u = pos.offset;
    if (u === undefined) u = x < tr.left ? 0 : row.text.length;
    return offsetAt(row, u);
  }

  private installPointer(): void {
    let pending: [number, number] | undefined;
    let raf = 0;
    this.viewport.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const off = this.hit(e.clientX, e.clientY);
      if (off === undefined) return;
      const vr = this.viewport.getBoundingClientRect();
      const inGutter = e.clientX - vr.left < this.gutterW;
      const unit = inGutter || e.detail >= 3 ? "line" : e.detail === 2 ? "word" : "char";
      const mode = e.shiftKey ? "extend" : e.altKey ? "add" : "set";
      this.send({ type: "click", offset: off, mode, unit });
      this.dragAnchor = mode === "set" && unit === "char" ? off : undefined;
      const move = (ev: MouseEvent) => {
        if (this.dragAnchor === undefined) return;
        const o = this.hit(ev.clientX, Math.min(Math.max(ev.clientY, vr.top), vr.bottom - 1));
        if (o === undefined) return;
        pending = [this.dragAnchor, o];
        // Auto-scroll at the edges.
        if (ev.clientY < vr.top) this.scrollByRows(-1);
        else if (ev.clientY > vr.bottom) this.scrollByRows(1);
        if (raf === 0) {
          raf = requestAnimationFrame(() => {
            raf = 0;
            if (pending) {
              this.send({ type: "setSelections", ranges: [pending] }, "drag");
              pending = undefined;
            }
          });
        }
      };
      const up = () => {
        this.dragAnchor = undefined;
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    });
  }

  private installScroll(): void {
    this.viewport.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const unit =
          e.deltaMode === WheelEvent.DOM_DELTA_LINE
            ? this.lineH
            : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
              ? this.viewH
              : 1;
        const dx = (e.shiftKey ? e.deltaY : e.deltaX) * unit;
        const dy = e.shiftKey ? 0 : e.deltaY * unit;
        if (dy !== 0) this.scrollByRows(dy / this.lineH);
        if (dx !== 0 && !this.wrap) this.scrollXBy(dx);
      },
      { passive: false },
    );
    // Vertical thumb drag maps to a fraction of the document.
    let grab: number | undefined;
    let seekTimer = 0;
    this.thumb.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.thumb.setPointerCapture(e.pointerId);
      grab = e.clientY - this.thumb.getBoundingClientRect().top;
    });
    this.thumb.addEventListener("pointermove", (e) => {
      if (grab === undefined) return;
      const track = this.track.getBoundingClientRect();
      const len = this.thumb.getBoundingClientRect().height;
      const f = Math.min(
        1,
        Math.max(0, (e.clientY - track.top - grab) / Math.max(1, track.height - len)),
      );
      this.thumb.style.transform = `translateY(${f * (track.height - len)}px)`;
      if (seekTimer === 0) {
        seekTimer = window.setTimeout(() => {
          seekTimer = 0;
          void this.seek(f);
        }, 16);
      }
    });
    const release = () => {
      grab = undefined;
    };
    this.thumb.addEventListener("pointerup", release);
    this.thumb.addEventListener("pointercancel", release);
    this.track.addEventListener("pointerdown", (e) => {
      const r = this.thumb.getBoundingClientRect();
      this.scrollByRows(e.clientY < r.top ? -this.visibleRows : this.visibleRows);
    });
    // Horizontal bar.
    let hgrab: number | undefined;
    this.hthumb.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      this.hthumb.setPointerCapture(e.pointerId);
      hgrab = e.clientX - this.hthumb.getBoundingClientRect().left;
    });
    this.hthumb.addEventListener("pointermove", (e) => {
      if (hgrab === undefined) return;
      const bar = this.hbar.getBoundingClientRect();
      const len = this.hthumb.getBoundingClientRect().width;
      const f = Math.min(
        1,
        Math.max(0, (e.clientX - bar.left - hgrab) / Math.max(1, bar.width - len)),
      );
      const contentW = this.maxCols * this.cellW - (this.viewW - this.gutterW);
      this.scrollXTo(f * Math.max(0, contentW));
    });
    this.hthumb.addEventListener("pointerup", () => {
      hgrab = undefined;
    });
  }

  private scrollXBy(dx: number): void {
    this.scrollXTo(this.scrollX + dx);
  }

  private scrollXTo(x: number): void {
    const max = Math.max(
      0,
      this.maxCols * this.cellW - (this.viewW - this.gutterW) + this.cellW * 4,
    );
    const next = Math.min(max, Math.max(0, x));
    if (next === this.scrollX) return;
    const oldCol = Math.floor(this.scrollX / this.cellW);
    this.scrollX = next;
    // Long rows carry only the text near the visible columns: refetch when
    // the window moves far.
    const hasLong = this.rows.some((r) => r.end - r.start > LONG_ROW_BYTES);
    if (hasLong && Math.abs(Math.floor(next / this.cellW) - oldCol) > 64) {
      this.refresh();
    }
    this.invalidateAll();
  }
}
