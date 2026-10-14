// Markdown preview surface (§6.2; ADR-0030): a virtualized view of rendered
// blocks. Imperative, outside React, like the TXT view.
//
// - The host renders blocks (sanitized HTML, ADR-0028) and sends windows of
//   them around a byte offset (`md_view`), with HTML only for blocks this
//   view doesn't have; after an edit it pushes the window again on the
//   document's Channel.
// - Only the window's blocks are in the DOM, in normal flow inside a layer
//   that is translated so the anchor (block, pixels into it) sits at the
//   viewport's top. Heights are measured from the DOM on each frame, so an
//   image or a late rendering that changes a block's height above the
//   anchor never moves what is on screen.
// - The scrollbar is a fraction of bytes (the document's height is never
//   known, and needn't be: M0's virtual scroll model, ADR-0005).

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import { normalize, topOffset } from "./anchor";
import { decodeWindow, type BlockMeta, type WindowMessage } from "./protocol";

const BEFORE = 40;
const AFTER = 80;
/** Fetch when the anchor is this close to either end of the window. */
const EDGE = 12;
/** Rendered blocks kept beyond the window (scrolling back is instant). */
const KEEP_HTML = 3000;
const LINE_ESTIMATE = 22;
/** Placing a block costs its innerHTML and its layout. A frame places what
 *  covers the viewport (plus this many viewports above and below), then
 *  grows the placed range toward the window's ends within GROW_MS, so no
 *  frame lays out a whole window (§4: no UI-thread task over 50 ms). */
const MARGIN_VIEWPORTS = 0.5;
const GROW_MS = 6;
const GROW_BATCH = 4;

export interface PreviewCallbacks {
  /** The viewport's top moved (scroll sync): byte offset at the top. */
  onScroll?(offset: number): void;
  /** A block was clicked (click-to-source): byte offset in the source. */
  onSourceClick?(offset: number): void;
  /** Remote images were blocked in what is shown. */
  onBlocked?(): void;
  onError(message: string): void;
}

/** Sample of keystroke → preview timing, for the benchmarks. */
export interface PatchSample {
  revision: number;
  /** Time the window arrived (performance.now()). */
  arrived: number;
  /** Time of the frame that shows it. */
  painted: number;
}

interface Placed {
  el: HTMLDivElement;
  rev: number;
}

export class MdPreview {
  readonly element: HTMLDivElement;
  private readonly viewport: HTMLDivElement;
  private readonly layer: HTMLDivElement;
  private readonly track: HTMLDivElement;
  private readonly thumb: HTMLDivElement;
  /** Rendered HTML by block id (latest revision received). */
  private readonly html = new Map<number, { rev: number; html: string }>();
  private readonly placed = new Map<number, Placed>();
  private readonly patchListeners = new Set<(s: PatchSample) => void>();
  private win: BlockMeta[] = [];
  private len = 0;
  private atStart = false;
  private atEnd = false;
  revision = 0;
  /** Anchor: block id, its start (to find it again after edits) and px. */
  private anchorId = -1;
  private anchorStart = 0;
  private anchorPx = 0;
  /** Measured heights by window index, for the placed range only. */
  private heights: (number | undefined)[] = [];
  /** The placed range of window indices [lo, hi): those blocks are in the
   *  layer, in order; the layer's top is block `lo`'s top. */
  private lo = 0;
  private hi = 0;
  /** Last measured height by block id and revision (estimates for blocks
   *  not placed; cleared on a width change). */
  private measured = new Map<number, { rev: number; h: number }>();
  private viewH = 0;
  private viewW = 0;
  private seq = 1;
  private fetching = false;
  private pending:
    { offset: number; px: number; jump: boolean; waiters: (() => void)[] } | undefined;
  private forget: number[] = [];
  private frame = 0;
  private dirty = false;
  private destroyed = false;
  private pendingPatch: { revision: number; arrived: number } | undefined;
  private resizeObserver: ResizeObserver;
  /** Set on a width change: the anchor's fraction of its block, applied to
   *  the block's new height on the next frame (reflowed text keeps its place). */
  private anchorFrac: number | undefined;
  private lastScrollOffset = -1;
  private reportTimer = 0;
  private reportedBytes = -1;
  /** Whether a frame ever showed rendered blocks (first paint marks). */
  private painted = false;

  constructor(
    host: HTMLElement,
    readonly doc: number,
    private readonly imagePrefix: string,
    private readonly cb: PreviewCallbacks,
  ) {
    this.element = document.createElement("div");
    this.element.className = "md-preview";
    this.viewport = document.createElement("div");
    this.viewport.className = "md-viewport";
    this.viewport.tabIndex = 0;
    this.viewport.setAttribute("role", "document");
    this.viewport.setAttribute("aria-label", t("md.preview"));
    this.layer = document.createElement("div");
    this.layer.className = "md-layer markdown-body";
    this.viewport.append(this.layer);
    this.track = document.createElement("div");
    this.track.className = "surface-scrollbar";
    this.thumb = document.createElement("div");
    this.thumb.className = "surface-thumb";
    this.track.append(this.thumb);
    this.element.append(this.viewport, this.track);
    host.append(this.element);
    this.installInput();
    this.resizeObserver = new ResizeObserver(() => {
      this.resize();
    });
    this.resizeObserver.observe(this.element);
    this.resize();
  }

  /** A window the host pushed (after an edit, or the index completing). */
  receive(buf: ArrayBuffer): void {
    this.onPush(buf);
  }

  onPatch(l: (s: PatchSample) => void): () => void {
    this.patchListeners.add(l);
    return () => this.patchListeners.delete(l);
  }

  focus(): void {
    this.viewport.focus({ preventScroll: true });
  }

  /** Byte offset at the viewport's top. */
  topOffset(): number {
    const i = this.anchorIndex();
    const b = this.win[i];
    if (!b) return 0;
    return topOffset(b.start, b.end, this.anchorPx, this.heights[i] ?? 0);
  }

  /** Shows the block holding `offset` at the top, `frac` of the way into it. */
  async reveal(offset: number, frac = 0): Promise<void> {
    await this.fetch(offset, frac, true);
  }

  /** Scrolls so `offset` is at the top, interpolating within its block. */
  scrollToOffset(offset: number): void {
    const i = this.win.findIndex((b) => offset >= b.start && offset < b.end);
    const b = this.win[i];
    const h = this.heights[i];
    if (b && h !== undefined && this.placed.has(b.id)) {
      this.anchorId = b.id;
      this.anchorStart = b.start;
      this.anchorPx = ((offset - b.start) / Math.max(1, b.end - b.start)) * h;
      this.lastScrollOffset = offset;
      this.invalidate();
      this.maybeFetch();
    } else {
      void this.fetch(offset, 0, true);
    }
  }

  /** Re-requests the current window (after the image policy changed). */
  refresh(): void {
    void this.fetch(this.anchorStart, this.anchorPx, false);
  }

  /** Blocks on screen (tests): id, start, end and text. */
  visibleBlocks(): { start: number; end: number; text: string }[] {
    const out: { start: number; end: number; text: string }[] = [];
    const top = this.layerTop();
    let y = 0;
    for (let i = this.lo; i < this.hi; i++) {
      const h = this.heights[i] ?? 0;
      const b = this.win[i]!;
      if (y + h > top && y < top + this.viewH) {
        const el = this.placed.get(b.id)?.el;
        out.push({ start: b.start, end: b.end, text: el?.textContent ?? "" });
      }
      y += h;
    }
    return out;
  }

  /** Blocks in the window still waiting for their HTML. */
  pendingBlocks(): number {
    return this.win.filter((b) => !this.html.has(b.id)).length;
  }

  scrollBy(px: number): void {
    if (this.win.length === 0) return;
    this.anchorPx += px;
    this.invalidate();
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.frame);
    this.resizeObserver.disconnect();
    void invoke("memory_release", { cache: "md-preview-dom", key: this.doc });
    void invoke("memory_release", { cache: "preview-images", key: this.doc });
    this.element.remove();
  }

  // ---- fetching ----------------------------------------------------------------

  private fetch(offset: number, px: number, jump: boolean): Promise<void> {
    return new Promise((resolve) => {
      const p = this.pending;
      if (p?.jump && !jump) p.waiters.push(resolve);
      else this.pending = { offset, px, jump, waiters: [...(p?.waiters ?? []), resolve] };
      if (!this.fetching) void this.runFetches();
    });
  }

  private async runFetches(): Promise<void> {
    this.fetching = true;
    try {
      for (let job = this.pending; job; job = this.pending) {
        this.pending = undefined;
        const seq = this.seq++;
        const forget = this.forget;
        this.forget = [];
        try {
          const buf = await invoke<ArrayBuffer>("md_view", {
            doc: this.doc,
            seq,
            offset: job.offset,
            before: BEFORE,
            after: AFTER,
            forget: forget.length > 0 ? forget : null,
          });
          if (this.destroyed) return;
          const m = decodeWindow(buf);
          // A window pushed after an edit may have overtaken this answer.
          if (m.revision < this.revision) {
            void this.fetch(job.offset, job.px, job.jump);
          } else {
            this.adopt(m, job.jump ? { px: job.px } : undefined);
          }
        } catch (err) {
          this.cb.onError(String(err));
        }
        for (const w of job.waiters) w();
      }
    } finally {
      this.fetching = false;
    }
  }

  private onPush(buf: ArrayBuffer): void {
    if (this.destroyed) return;
    let m: WindowMessage;
    try {
      m = decodeWindow(buf);
    } catch (err) {
      this.cb.onError(String(err));
      return;
    }
    if (m.revision < this.revision) return;
    this.pendingPatch = { revision: m.revision, arrived: performance.now() };
    this.adopt(m, undefined);
  }

  /** Takes a window. `jump`: show its anchor block at `px` (else keep the
   *  current anchor, found by id or by its start offset). */
  private adopt(m: WindowMessage, jump: { px: number } | undefined): void {
    for (const b of m.blocks) {
      if (b.html !== undefined) this.html.set(b.id, { rev: b.rev, html: b.html });
    }
    // An edit can also arrive in a requested window (md_view applies a
    // pending change, and the worker then has nothing to push).
    if (m.revision > this.revision && !this.pendingPatch) {
      this.pendingPatch = { revision: m.revision, arrived: performance.now() };
    }
    this.revision = Math.max(this.revision, m.revision);
    this.len = m.len;
    this.atStart = m.atStart;
    this.atEnd = m.atEnd;
    const prevAnchor = this.anchorId;
    this.win = m.blocks;
    // The placed range in the new window: the span of blocks already placed
    // (an edited block inside it, with a new id, is placed by draw()).
    let lo = -1;
    let hi = -1;
    m.blocks.forEach((b, i) => {
      if (this.placed.has(b.id)) {
        if (lo < 0) lo = i;
        hi = i + 1;
      }
    });
    this.lo = Math.max(0, lo);
    this.hi = Math.max(0, hi);
    this.heights = m.blocks.map((b) =>
      this.placed.has(b.id) ? this.measured.get(b.id)?.h : undefined,
    );
    if (this.measured.size > 4 * m.blocks.length) {
      const keep = new Set(m.blocks.map((b) => b.id));
      for (const id of this.measured.keys()) if (!keep.has(id)) this.measured.delete(id);
    }
    const anchor = m.blocks[m.anchor];
    if (jump && anchor) {
      this.anchorId = anchor.id;
      this.anchorStart = anchor.start;
      this.anchorPx = jump.px;
    } else if (!m.blocks.some((b) => b.id === prevAnchor)) {
      // The anchor block was edited (a new id): the block now at its start.
      const at =
        m.blocks.find((b) => this.anchorStart >= b.start && this.anchorStart < b.end) ?? anchor;
      if (at) {
        this.anchorId = at.id;
        this.anchorStart = at.start;
      }
    }
    // Blocks we lack HTML for (dropped earlier): ask again.
    const missing = m.blocks.filter(
      (b) => !this.html.has(b.id) || this.html.get(b.id)!.rev !== b.rev,
    );
    if (missing.length > 0) {
      this.forget.push(...missing.map((b) => b.id));
      if (!this.pending) void this.fetch(this.anchorStart, this.anchorPx, false);
    }
    this.trimHtml();
    this.invalidate();
  }

  private trimHtml(): void {
    if (this.html.size <= KEEP_HTML + this.win.length) return;
    const keep = new Set(this.win.map((b) => b.id));
    for (const id of this.html.keys()) {
      if (this.html.size <= KEEP_HTML) break;
      if (!keep.has(id)) {
        this.html.delete(id);
        this.forget.push(id);
      }
    }
  }

  private anchorIndex(): number {
    const i = this.win.findIndex((b) => b.id === this.anchorId);
    return i < 0 ? 0 : i;
  }

  private atDocStart(): boolean {
    return this.atStart;
  }

  private atDocEnd(): boolean {
    return this.atEnd;
  }

  private maybeFetch(): void {
    const i = this.anchorIndex();
    const nearStart = i < EDGE && !this.atDocStart();
    const nearEnd = this.win.length - i < EDGE * 2 && !this.atDocEnd();
    if ((nearStart || nearEnd) && !this.pending) {
      void this.fetch(this.anchorStart, this.anchorPx, false);
    }
  }

  // ---- drawing -------------------------------------------------------------------

  private resize(): void {
    const r = this.viewport.getBoundingClientRect();
    const widthChanged = Math.abs(r.width - this.viewW) > 0.5;
    this.viewW = r.width;
    this.viewH = r.height;
    if (this.win.length === 0) void this.fetch(0, 0, true);
    else if (widthChanged) {
      const h = this.heights[this.anchorIndex()];
      if (h && h > 0 && this.anchorFrac === undefined) this.anchorFrac = this.anchorPx / h;
      this.measured.clear();
      // Images are scaled to the width: let new ones load at the new size.
      for (const p of this.placed.values()) p.el.dataset.images = "";
      this.invalidate();
    } else this.invalidate();
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

  /** The viewport's top in the layer (whose top is block `lo`'s top). */
  private layerTop(): number {
    const i = this.anchorIndex();
    let y = 0;
    for (let k = this.lo; k < i; k++) y += this.heights[k] ?? 0;
    return y + this.anchorPx;
  }

  /** A block's height: measured (same rendering) or from its line count. */
  private estimate(i: number): number {
    const b = this.win[i]!;
    const m = this.measured.get(b.id);
    if (m && m.rev === (this.html.get(b.id)?.rev ?? -1)) return m.h;
    return Math.max(1, b.lines) * LINE_ESTIMATE;
  }

  /** Makes the layer hold exactly the window's blocks [lo, hi), in order,
   *  and measures them (one layout). */
  private place(lo: number, hi: number): void {
    const want = new Set<number>();
    for (let i = lo; i < hi; i++) want.add(this.win[i]!.id);
    for (const [id, p] of this.placed) {
      if (!want.has(id)) {
        p.el.remove();
        this.placed.delete(id);
      }
    }
    let prev: HTMLElement | null = null;
    for (let i = lo; i < hi; i++) {
      const el = this.blockEl(this.win[i]!);
      const next: ChildNode | null = prev ? prev.nextSibling : this.layer.firstChild;
      if (next !== el) this.layer.insertBefore(el, next);
      prev = el;
    }
    this.lo = lo;
    this.hi = hi;
    this.heights = new Array<number | undefined>(this.win.length);
    for (let i = lo; i < hi; i++) {
      const b = this.win[i]!;
      const p = this.placed.get(b.id)!;
      const h = p.el.offsetHeight;
      this.heights[i] = h;
      this.measured.set(b.id, { rev: p.rev, h });
    }
  }

  private blockEl(b: BlockMeta): HTMLDivElement {
    const have = this.html.get(b.id);
    let p = this.placed.get(b.id);
    if (!p) {
      const el = document.createElement("div");
      el.className = "md-block";
      p = { el, rev: -1 };
      this.placed.set(b.id, p);
    }
    p.el.dataset.start = String(b.start);
    p.el.dataset.end = String(b.end);
    if (have && p.rev !== have.rev) {
      p.rev = have.rev;
      p.el.classList.remove("pending");
      p.el.style.minHeight = "";
      p.el.innerHTML = have.html;
      p.el.dataset.images = "";
      this.decorate(p.el);
    } else if (!have && p.rev === -1) {
      // A placeholder of about the right height until the HTML arrives
      // (§7: never a blank screen).
      p.el.classList.add("pending");
      p.el.style.minHeight = `${b.lines * LINE_ESTIMATE}px`;
      p.el.textContent = "";
    }
    return p.el;
  }

  /** Placeholders for blocked images, link handling. */
  private decorate(el: HTMLElement): void {
    for (const big of el.querySelectorAll<HTMLElement>(".md-too-large")) {
      const label = document.createElement("div");
      label.className = "md-too-large-label";
      label.textContent = t("md.tooLarge", {
        size: Math.round(Number(big.dataset.bytes ?? 0) / 1024),
      });
      big.prepend(label);
    }
    let blocked = false;
    for (const img of el.querySelectorAll<HTMLImageElement>("img[data-blocked-src]")) {
      blocked = true;
      const src = img.dataset.blockedSrc ?? "";
      const ph = document.createElement("span");
      ph.className = "md-image-blocked";
      ph.title = src;
      ph.textContent = img.alt
        ? t("md.imageBlockedAlt", { alt: img.alt, src })
        : t("md.imageBlocked", { src });
      img.replaceWith(ph);
    }
    if (blocked) this.cb.onBlocked?.();
  }

  /** Starts loading images in placed blocks at the display width. */
  private loadImages(el: HTMLElement): void {
    if (el.dataset.images === String(Math.round(this.viewW))) return;
    el.dataset.images = String(Math.round(this.viewW));
    const w = Math.max(1, Math.round(this.viewW * window.devicePixelRatio));
    for (const img of el.querySelectorAll<HTMLImageElement>("img[data-src]")) {
      const src = img.dataset.src ?? "";
      // Only our scheme: the sanitizer writes these, never the document.
      if (!src.startsWith(this.imagePrefix)) continue;
      const url = `${src}?w=${w}`;
      if (img.getAttribute("src") === url) continue;
      img.decoding = "async";
      img.onerror = () => {
        const ph = document.createElement("span");
        ph.className = "md-image-blocked error";
        ph.textContent = t("md.imageFailed", { alt: img.alt || src });
        img.replaceWith(ph);
        this.invalidate();
      };
      img.onload = () => {
        this.invalidate();
        this.reportMemory();
      };
      img.src = url;
    }
  }

  private draw(): void {
    if (!this.dirty || this.destroyed) return;
    // Hidden (the editing view): every block would measure 0 px and the
    // anchor would run to the window's end. Wait to be shown (the resize
    // observer draws again then).
    if (this.viewH === 0 || this.viewport.offsetParent === null) return;
    this.dirty = false;
    const t0 = performance.now();
    const n = this.win.length;
    const ai = this.anchorIndex();
    // 1. Place what covers the viewport and a margin (from estimates),
    //    keeping the placed range when it touches that.
    const margin = this.viewH * MARGIN_VIEWPORTS;
    let lo = ai;
    for (let need = margin - this.anchorPx; need > 0 && lo > 0;) need -= this.estimate(--lo);
    let hi = ai;
    for (let y = -this.anchorPx; hi < n && (hi === ai || y < this.viewH + margin); hi++) {
      y += this.estimate(hi);
    }
    if (this.hi > this.lo && lo <= this.hi && hi >= this.lo) {
      lo = Math.min(lo, this.lo);
      hi = Math.max(hi, this.hi);
    }
    this.place(lo, hi);
    if (this.anchorFrac !== undefined) {
      this.anchorPx = this.anchorFrac * (this.heights[ai] ?? 0);
      this.anchorFrac = undefined;
    }
    // 2. The anchor onto the block at the viewport's top; if the estimates
    //    fell short, place more and again.
    let a: { index: number; px: number };
    for (let pass = 0; ; pass++) {
      const r = normalize(
        { index: ai - this.lo, px: this.anchorPx },
        this.heights.slice(this.lo, this.hi) as number[],
        this.atDocStart() && this.lo === 0,
        this.atDocEnd() && this.hi === n,
        this.viewH,
      );
      a = { index: r.index + this.lo, px: r.px };
      let below = -a.px;
      for (let i = a.index; i < this.hi; i++) below += this.heights[i] ?? 0;
      const shortAbove = a.px < 0 && this.lo > 0;
      const shortBelow = below < this.viewH && this.hi < n;
      if ((!shortAbove && !shortBelow) || pass >= 8) break;
      this.place(
        shortAbove ? Math.max(0, this.lo - GROW_BATCH) : this.lo,
        shortBelow ? Math.min(n, this.hi + GROW_BATCH) : this.hi,
      );
    }
    const ab = this.win[a.index];
    if (ab) {
      this.anchorId = ab.id;
      this.anchorStart = ab.start;
    }
    this.anchorPx = a.px;
    // 3. Grow the placed range toward the window's ends, a few blocks at a
    //    time within the frame's budget; the rest on later frames.
    let below = true;
    while ((this.lo > 0 || this.hi < n) && performance.now() - t0 < GROW_MS) {
      if ((below && this.hi < n) || this.lo === 0) {
        this.place(this.lo, Math.min(n, this.hi + GROW_BATCH));
      } else {
        this.place(Math.max(0, this.lo - GROW_BATCH), this.hi);
      }
      below = !below;
    }
    if (this.lo > 0 || this.hi < n) this.invalidate();
    this.layer.style.transform = `translateY(${-this.layerTop()}px)`;
    // 4. Images near the viewport.
    const top = this.layerTop();
    let y = 0;
    for (let i = this.lo; i < this.hi; i++) {
      const h = this.heights[i] ?? 0;
      if (y + h > top - this.viewH && y < top + 2 * this.viewH) {
        const el = this.placed.get(this.win[i]!.id)?.el;
        if (el) this.loadImages(el);
      }
      y += h;
    }
    this.updateScrollbar();
    this.maybeFetch();
    const off = this.topOffset();
    if (off !== this.lastScrollOffset) {
      this.lastScrollOffset = off;
      this.cb.onScroll?.(off);
    }
    if (!this.painted && this.win.some((b) => this.html.has(b.id))) {
      this.painted = true;
      performance.mark("md-preview-first-content");
    }
    const patch = this.pendingPatch;
    if (patch) {
      this.pendingPatch = undefined;
      const painted = performance.now();
      for (const l of this.patchListeners) l({ ...patch, painted });
    }
    this.reportMemory();
  }

  private updateScrollbar(): void {
    const track = this.track.clientHeight;
    if (this.len === 0 || track === 0) {
      this.thumb.style.display = "none";
      return;
    }
    let shown = 0;
    let y = -this.anchorPx;
    for (let i = this.anchorIndex(); i < this.hi && y < this.viewH; i++) {
      const b = this.win[i]!;
      shown += b.end - b.start;
      y += this.heights[i] ?? 0;
    }
    const length = Math.min(track, Math.max(24, (track * shown) / this.len));
    const f = Math.min(1, this.topOffset() / this.len);
    this.thumb.style.display = this.atDocStart() && this.atDocEnd() && y < this.viewH ? "none" : "";
    this.thumb.style.height = `${length}px`;
    this.thumb.style.transform = `translateY(${f * (track - length)}px)`;
  }

  /** Reports the DOM blocks' and decoded images' sizes (§5.5). */
  private reportMemory(): void {
    if (this.reportTimer !== 0) return;
    this.reportTimer = window.setTimeout(() => {
      this.reportTimer = 0;
      if (this.destroyed) return;
      let html = 0;
      for (const v of this.html.values()) html += v.html.length * 2;
      if (html !== this.reportedBytes) {
        this.reportedBytes = html;
        void invoke("memory_report", {
          cache: "md-preview-dom",
          key: this.doc,
          doc: this.doc,
          bytes: html,
        });
      }
      let images = 0;
      for (const img of this.layer.querySelectorAll("img")) {
        images += img.naturalWidth * img.naturalHeight * 4;
      }
      void invoke("memory_report", {
        cache: "preview-images",
        key: this.doc,
        doc: this.doc,
        bytes: images,
      });
    }, 250);
  }

  // ---- input ---------------------------------------------------------------------

  private installInput(): void {
    this.viewport.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const unit =
          e.deltaMode === WheelEvent.DOM_DELTA_LINE
            ? LINE_ESTIMATE
            : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
              ? this.viewH
              : 1;
        this.scrollBy(e.deltaY * unit);
      },
      { passive: false },
    );
    this.viewport.addEventListener("keydown", (e) => {
      const page = this.viewH * 0.9;
      const step: Record<string, number> = {
        ArrowDown: 40,
        ArrowUp: -40,
        PageDown: page,
        PageUp: -page,
        " ": e.shiftKey ? -page : page,
      };
      if (e.key in step) {
        e.preventDefault();
        this.scrollBy(step[e.key]!);
      } else if (e.key === "Home" && e.ctrlKey) {
        e.preventDefault();
        void this.reveal(0);
      } else if (e.key === "End" && e.ctrlKey) {
        e.preventDefault();
        void this.reveal(Math.max(0, this.len - 1), 1e9);
      }
    });
    // Links never navigate the webview (§8). In-document anchors scroll to
    // their target; a click elsewhere (with the source open) moves its caret.
    this.layer.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      const a = target.closest("a");
      if (a) {
        e.preventDefault();
        const href = a.getAttribute("href") ?? "";
        if (href.startsWith("#")) this.jumpToFragment(href.slice(1));
        return;
      }
      if (window.getSelection()?.isCollapsed === false) return;
      const block = target.closest<HTMLElement>(".md-block");
      if (!block || !this.cb.onSourceClick) return;
      this.cb.onSourceClick(this.sourceOffsetAt(block, e.clientY));
    });
    // Thumb drag maps to a byte fraction.
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
          void this.reveal(Math.floor(f * this.len));
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
      this.scrollBy(e.clientY < r.top ? -this.viewH * 0.9 : this.viewH * 0.9);
    });
  }

  /** Source offset for a click at `clientY` in a block: the line of the
   *  element with a source position under the pointer, or an interpolation. */
  private sourceOffsetAt(block: HTMLElement, clientY: number): number {
    const start = Number(block.dataset.start ?? 0);
    const end = Number(block.dataset.end ?? start);
    const r = block.getBoundingClientRect();
    const f = r.height > 0 ? Math.min(1, Math.max(0, (clientY - r.top) / r.height)) : 0;
    return Math.floor(start + f * (end - start));
  }

  private jumpToFragment(id: string): void {
    const el = this.layer.querySelector(`[id="${CSS.escape(decodeURIComponent(id))}"]`);
    const block = el?.closest<HTMLElement>(".md-block");
    if (block) void this.reveal(Number(block.dataset.start ?? 0));
  }
}
