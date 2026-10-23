// The PDF view (M3 tasks 4–5; §5.4, §7; ADR-0038).
//
// Imperative, outside React. One canvas: a gray desk, white page frames
// (never blank: a page without tiles yet shows its frame and, as soon as
// it has arrived, a low-resolution bitmap of the whole page), then sharp
// page-local tiles from the core. While zooming, tiles of the previous
// scale are drawn stretched until sharp ones replace them.

import { Channel, invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import type { DecodedTile } from "../surface/decode-worker";
import { tileKey, type TileId } from "../surface/protocol";
import { ScrollModel } from "../surface/scroll-model";
import {
  TileCache,
  registerTileReceiver,
  unregisterTileReceiver,
  type TileCounters,
  type TileReceiver,
} from "../surface/surface";
import { PdfLayout, fitPageScale, fitWidthScale, type LayoutMode, type PageBox } from "./layout";

export const TILE = 512;
/** Longest side of a page's low-resolution fallback bitmap, device px. */
const FALLBACK_PX = 384;
const MIN_ZOOM = 10;
const MAX_ZOOM = 6400;
const DESK = "#d9dce3";
const LINE_CSS_PX = 40;
const SCROLLBAR_CSS_PX = 12;

export type ZoomMode = "fitWidth" | "fitPage" | "custom";

export interface PdfOpened {
  doc: number;
  pages: number;
  validXref: boolean;
  version: number;
  firstSize: [number, number];
  transport: string;
  openMs: number;
}

export interface PdfViewState {
  page: number;
  pages: number;
  /** Page sizes still being learned: the total height is an estimate. */
  estimated: boolean;
  zoom: number;
  zoomMode: ZoomMode;
  mode: LayoutMode;
  rotation: number;
  failedPages: number;
}

type PdfEvent =
  | { kind: "sizes"; from: number; sizes: number[] }
  | { kind: "sizesDone"; ms: number }
  | { kind: "walkFailed"; from: number; error: string }
  | { kind: "pageFailed"; page: number; error: string };

/** Mirrors `quire_pdf::tiles::layout_id` (scale as f32 bits). */
export function layoutId(scale: number, rotation: number, revision = 0): number {
  const bits = new DataView(new ArrayBuffer(4));
  bits.setFloat32(0, scale, true);
  const scaleBits = bits.getUint32(0, true);
  return (revision & 0x7ffff) * 2 ** 34 + scaleBits * 4 + ((rotation / 90) % 4);
}

/** Mirrors `quire_pdf::tiles::row_id`. */
export function rowId(page: number, row: number): number {
  return page * 2 ** 24 + (row & 0xffffff);
}

/** Rounds a scale to what the core will use (f32). */
function f32(x: number): number {
  return Math.fround(x);
}

export class PdfView implements TileReceiver {
  readonly element: HTMLDivElement;
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D;
  private readonly vtrack: HTMLDivElement;
  private readonly vthumb: HTMLDivElement;
  private readonly worker: Worker;
  private readonly cache = new TileCache(256);
  readonly counters: TileCounters = { received: 0, droppedUnwanted: 0, requests: 0 };
  private readonly listeners = new Set<(s: PdfViewState) => void>();
  private readonly resizeObserver: ResizeObserver;
  private readonly scrollY = new ScrollModel(0, 0);
  private scrollX = 0;

  private info: PdfOpened | undefined;
  private sizes = new Float32Array(0);
  private estimated = true;
  private readonly failed = new Map<number, string>();
  private layout: PdfLayout | undefined;
  private mode: LayoutMode = "continuous";
  private rotation = 0;
  private zoom = 100;
  private zoomMode: ZoomMode = "fitWidth";
  private scale = 1;
  /** The scale before the last zoom, whose tiles stand in meanwhile. */
  private staleScale: number | undefined;
  private single = 0;
  private dpr = window.devicePixelRatio;
  private width = 0;
  private height = 0;
  private frameHandle = 0;
  private dirty = true;
  private requestPending = true;
  private lastRequestAt = 0;
  private reportTimer = 0;
  private reportedBytes = 0;

  constructor(host: HTMLElement) {
    this.element = document.createElement("div");
    this.element.className = "surface pdf-view";
    this.element.tabIndex = 0;
    this.canvas = document.createElement("canvas");
    this.vtrack = document.createElement("div");
    this.vtrack.className = "surface-scrollbar";
    this.vthumb = document.createElement("div");
    this.vthumb.className = "surface-thumb";
    this.vtrack.append(this.vthumb);
    this.element.append(this.canvas, this.vtrack);
    host.append(this.element);
    this.ctx = this.canvas.getContext("2d", { alpha: false })!;
    this.worker = new Worker(new URL("../surface/decode-worker.ts", import.meta.url), {
      type: "module",
    });
    this.worker.onmessage = (e: MessageEvent<DecodedTile>) => {
      this.receive(e.data.id, e.data.bitmap);
    };
    this.installInput();
    this.resizeObserver = new ResizeObserver(() => {
      this.resize();
    });
    this.resizeObserver.observe(this.element);
    this.resize();
  }

  /** Opens `path`; rejects with "password" when one is needed or wrong. */
  async open(path: string, password?: string): Promise<PdfOpened> {
    const tiles = new Channel<ArrayBuffer>();
    tiles.onmessage = (buf) => {
      this.worker.postMessage(buf, [buf]);
    };
    // The page walk can report before `pdf_open` resolves: hold its events
    // until the document's arrays exist.
    const early: PdfEvent[] = [];
    let ready = false;
    const events = new Channel<PdfEvent>();
    events.onmessage = (e) => {
      if (ready) this.onEvent(e);
      else early.push(e);
    };
    const info = await invoke<PdfOpened>("pdf_open", {
      path,
      password: password ?? null,
      tiles,
      events,
    });
    this.info = info;
    this.sizes = new Float32Array(2 * info.pages);
    for (let i = 0; i < info.pages; i++) {
      this.sizes[2 * i] = info.firstSize[0];
      this.sizes[2 * i + 1] = info.firstSize[1];
    }
    this.estimated = info.pages > 1;
    registerTileReceiver(info.doc, this);
    this.applyZoomMode();
    this.relayout(undefined);
    ready = true;
    for (const e of early) this.onEvent(e);
    return info;
  }

  get opened(): PdfOpened | undefined {
    return this.info;
  }

  onChange(listener: (s: PdfViewState) => void): () => void {
    this.listeners.add(listener);
    listener(this.state());
    return () => this.listeners.delete(listener);
  }

  state(): PdfViewState {
    return {
      page: this.currentPage(),
      pages: this.info?.pages ?? 0,
      estimated: this.estimated,
      zoom: this.zoom,
      zoomMode: this.zoomMode,
      mode: this.mode,
      rotation: this.rotation,
      failedPages: this.failed.size,
    };
  }

  currentPage(): number {
    if (this.mode === "single") return this.single;
    return this.layout?.pageAt(this.scrollY.value + this.height / 3) ?? 0;
  }

  // ---- commands ---------------------------------------------------------------

  gotoPage(page: number): void {
    const n = this.info?.pages ?? 0;
    if (n === 0) return;
    const p = Math.min(Math.max(0, Math.round(page)), n - 1);
    if (this.mode === "single") {
      this.single = p;
      this.relayout(undefined);
      this.scrollY.scrollTo(0);
    } else {
      const box = this.layout?.box(p);
      if (box) this.scrollY.scrollTo(box.y - (this.layout?.input.gap ?? 0));
    }
    this.invalidate(true);
    this.emit();
  }

  setZoom(zoom: number, anchor?: { x: number; y: number }): void {
    this.zoomMode = "custom";
    this.zoomTo(zoom, anchor);
  }

  zoomBy(factor: number, anchor?: { x: number; y: number }): void {
    this.setZoom(this.zoom * factor, anchor);
  }

  fit(mode: "fitWidth" | "fitPage"): void {
    this.zoomMode = mode;
    const anchor = this.anchorPage();
    this.applyZoomMode();
    this.relayout(anchor);
  }

  setMode(mode: LayoutMode): void {
    if (mode === this.mode) return;
    const page = this.currentPage();
    this.mode = mode;
    this.single = page;
    this.applyZoomMode();
    this.relayout(undefined);
    this.gotoPage(page);
  }

  rotate(delta: 90 | -90): void {
    const anchor = this.anchorPage();
    this.rotation = (this.rotation + delta + 360) % 360;
    this.applyZoomMode();
    this.relayout(anchor);
  }

  // ---- tiles ------------------------------------------------------------------

  wants(id: TileId): boolean {
    const info = this.info;
    if (!info || id.view !== info.doc || id.size !== TILE) return false;
    const page = Math.floor(id.row / 2 ** 24);
    if (id.layout === layoutId(f32(this.fallbackScale(page)), this.rotation)) return true;
    return id.layout === layoutId(f32(this.scale), this.rotation);
  }

  receive(id: TileId, bitmap: ImageBitmap): void {
    if (!this.wants(id)) {
      this.counters.droppedUnwanted++;
      bitmap.close();
      return;
    }
    this.counters.received++;
    this.cache.put(tileKey(id), bitmap);
    this.reportMemory();
    this.invalidate(false);
  }

  trimCache(bytes: number): void {
    this.cache.trim(bytes);
    this.reportMemory();
    this.invalidate(true);
  }

  async close(): Promise<void> {
    cancelAnimationFrame(this.frameHandle);
    clearTimeout(this.reportTimer);
    this.resizeObserver.disconnect();
    this.worker.terminate();
    this.cache.clear();
    const info = this.info;
    if (info) {
      unregisterTileReceiver(info.doc);
      await Promise.all([
        invoke("pdf_close", { doc: info.doc }),
        invoke("memory_release", { cache: "tile-bitmaps", key: info.doc }),
      ]);
    }
    this.element.remove();
  }

  /** Whether the viewport is drawn with sharp tiles and no frame is pending. */
  settled(): boolean {
    return this.frameHandle === 0 && !this.dirty && this.missingVisible() === 0;
  }

  /** Number of tiles at the current scale missing from the viewport. */
  missingVisible(): number {
    let missing = 0;
    for (const b of this.visiblePages()) {
      for (const t of this.tilesOf(b, 0)) {
        if (!this.cache.has(this.key(b.page, t.col, t.row, this.scale))) missing++;
      }
    }
    return missing;
  }

  /** Where `page` is on screen, in device px of the canvas (tests). */
  pageOnScreen(page: number): { x: number; y: number; width: number; height: number } | undefined {
    const b = this.layout?.box(page);
    if (!b) return undefined;
    return { x: b.x - this.scrollX, y: b.y - this.scrollY.value, width: b.width, height: b.height };
  }

  /** The canvas pixel at device px (x, y) as [r, g, b] (tests). */
  pixel(x: number, y: number): [number, number, number] {
    const d = this.ctx.getImageData(Math.round(x), Math.round(y), 1, 1).data;
    return [d[0]!, d[1]!, d[2]!];
  }

  /** PNG of the canvas, for visual checks in tests and benchmarks. */
  snapshot(): string {
    return this.canvas.toDataURL("image/png");
  }

  // ---- internals --------------------------------------------------------------

  private emit(): void {
    const s = this.state();
    for (const l of this.listeners) l(s);
  }

  private onEvent(e: PdfEvent): void {
    switch (e.kind) {
      case "sizes": {
        const anchor = this.anchorPage();
        this.sizes.set(e.sizes, 2 * e.from);
        this.relayout(anchor);
        break;
      }
      case "sizesDone":
        this.estimated = false;
        this.emit();
        break;
      case "walkFailed":
        console.warn("page walk failed", e.error);
        break;
      case "pageFailed":
        this.failed.set(e.page, e.error);
        this.invalidate(false);
        this.emit();
        break;
    }
  }

  /** The page at the top of the viewport and how far into it, to keep in place. */
  private anchorPage(): { page: number; frac: number } | undefined {
    const l = this.layout;
    if (!l) return undefined;
    const page = this.mode === "single" ? this.single : l.pageAt(this.scrollY.value);
    const b = l.box(page);
    if (!b) return undefined;
    return { page, frac: (this.scrollY.value - b.y) / b.height };
  }

  private fallbackScale(page: number): number {
    const w = this.sizes[2 * page] ?? 612;
    const h = this.sizes[2 * page + 1] ?? 792;
    return FALLBACK_PX / Math.max(w, h, 1);
  }

  private applyZoomMode(): void {
    if (!this.info || this.width === 0) return;
    const gap = this.gap();
    const page = this.currentPage();
    const s =
      this.zoomMode === "fitWidth"
        ? fitWidthScale(this.sizes, this.mode, this.rotation, this.width, gap, page)
        : this.zoomMode === "fitPage"
          ? fitPageScale(this.sizes, this.mode, this.rotation, this.width, this.height, gap, page)
          : undefined;
    if (s !== undefined) this.setScale(s);
  }

  /** Device px per point at `zoom` %. */
  private scaleFor(zoom: number): number {
    return (zoom / 100) * this.dpr * (96 / 72);
  }

  private setScale(scale: number): void {
    const zoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, (scale / this.scaleFor(100)) * 100));
    const next = this.scaleFor(zoom);
    if (next !== this.scale) {
      this.staleScale = this.scale;
      this.scale = next;
    }
    this.zoom = zoom;
  }

  private zoomTo(zoom: number, anchor?: { x: number; y: number }): void {
    const l = this.layout;
    const ax = anchor?.x ?? this.width / 2;
    const ay = anchor?.y ?? this.height / 2;
    // The point under the anchor, in page coordinates.
    const docX = this.scrollX + ax;
    const docY = this.scrollY.value + ay;
    const page = l ? l.pageAt(docY) : 0;
    const b = l?.box(page);
    const old = this.scale;
    this.setScale(this.scaleFor(Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom))));
    this.relayout(undefined);
    const nb = this.layout?.box(page);
    if (b && nb) {
      const r = this.scale / old;
      this.scrollX = this.clampX(nb.x + (docX - b.x) * r - ax);
      this.scrollY.scrollTo(nb.y + (docY - b.y) * r - ay);
    }
    this.invalidate(true);
    this.emit();
  }

  private gap(): number {
    return Math.round(12 * this.dpr);
  }

  private relayout(anchor: { page: number; frac: number } | undefined): void {
    if (!this.info) return;
    this.layout = new PdfLayout({
      sizes: this.sizes,
      mode: this.mode,
      rotation: this.rotation,
      scale: this.scale,
      gap: this.gap(),
      viewportWidth: this.width,
      current: this.single,
    });
    this.scrollY.setExtent(this.layout.height);
    this.scrollX = this.clampX(this.scrollX);
    if (anchor) {
      const b = this.layout.box(anchor.page);
      if (b) this.scrollY.scrollTo(b.y + anchor.frac * b.height);
    }
    this.invalidate(true);
    this.emit();
  }

  private clampX(x: number): number {
    const max = Math.max(0, (this.layout?.width ?? 0) - this.width);
    return Math.min(Math.max(0, x), max);
  }

  private resize(): void {
    this.dpr = window.devicePixelRatio;
    const rect = this.element.getBoundingClientRect();
    const cssWidth = Math.max(0, rect.width - SCROLLBAR_CSS_PX);
    this.width = Math.round(cssWidth * this.dpr);
    this.height = Math.round(rect.height * this.dpr);
    this.canvas.width = this.width;
    this.canvas.height = this.height;
    this.canvas.style.width = `${cssWidth}px`;
    this.canvas.style.height = `${rect.height}px`;
    this.scrollY.setViewport(this.height);
    this.cache.capacity = Math.max(
      256,
      Math.ceil(this.width / TILE + 1) * Math.ceil(this.height / TILE + 1) * 8,
    );
    const anchor = this.anchorPage();
    if (this.zoomMode !== "custom") this.applyZoomMode();
    else this.setScale(this.scaleFor(this.zoom));
    this.relayout(anchor);
  }

  private invalidate(viewportChanged: boolean): void {
    this.dirty = true;
    if (viewportChanged) this.requestPending = true;
    if (this.frameHandle === 0) {
      this.frameHandle = requestAnimationFrame(() => {
        this.frame();
      });
    }
  }

  private visiblePages(): PageBox[] {
    const l = this.layout;
    if (!l) return [];
    return l.pagesIn(this.scrollY.value, this.scrollY.value + this.height);
  }

  /** Tiles of a page box inside the viewport grown by `margin` device px. */
  private tilesOf(b: PageBox, margin: number): { col: number; row: number }[] {
    const x0 = Math.max(0, this.scrollX - margin - b.x);
    const x1 = Math.min(b.width, this.scrollX + this.width + margin - b.x);
    const y0 = Math.max(0, this.scrollY.value - margin - b.y);
    const y1 = Math.min(b.height, this.scrollY.value + this.height + margin - b.y);
    const out: { col: number; row: number }[] = [];
    if (x1 <= x0 || y1 <= y0) return out;
    for (let row = Math.floor(y0 / TILE); row * TILE < y1; row++) {
      for (let col = Math.floor(x0 / TILE); col * TILE < x1; col++) out.push({ col, row });
    }
    return out;
  }

  private key(page: number, col: number, row: number, scale: number): string {
    return tileKey({
      view: this.info?.doc ?? 0,
      layout: layoutId(f32(scale), this.rotation),
      size: TILE,
      col,
      row: rowId(page, row),
    });
  }

  private frame(): void {
    this.frameHandle = 0;
    if (!this.dirty) return;
    this.dirty = false;
    const ctx = this.ctx;
    ctx.fillStyle = DESK;
    ctx.fillRect(0, 0, this.width, this.height);
    const sx = this.scrollX;
    const sy = this.scrollY.value;
    let missing = 0;
    for (const b of this.visiblePages()) {
      const x = b.x - sx;
      const y = b.y - sy;
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(x, y, b.width, b.height);
      // Low-resolution whole page under everything else.
      const fb = this.cache.get(this.key(b.page, 0, 0, this.fallbackScale(b.page)));
      if (fb) ctx.drawImage(fb, x, y, b.width, b.height);
      // Tiles of the previous scale, stretched.
      if (this.staleScale !== undefined) this.drawStale(b, x, y, this.staleScale);
      for (const t of this.tilesOf(b, 0)) {
        const bmp = this.cache.get(this.key(b.page, t.col, t.row, this.scale));
        if (bmp) ctx.drawImage(bmp, x + t.col * TILE, y + t.row * TILE);
        else missing++;
      }
      const err = this.failed.get(b.page);
      if (err !== undefined) {
        ctx.fillStyle = "#b42318";
        ctx.font = `${Math.round(13 * this.dpr)}px system-ui, sans-serif`;
        ctx.fillText(t("pdf.pageFailed", { page: b.page + 1, error: err }), x + 16, y + 32);
      }
      ctx.strokeStyle = "#b9bdc7";
      ctx.lineWidth = 1;
      ctx.strokeRect(x - 0.5, y - 0.5, b.width + 1, b.height + 1);
    }
    if (missing === 0) this.staleScale = undefined;
    this.updateThumb();
    const now = performance.now();
    if (this.requestPending || (missing > 0 && now - this.lastRequestAt > 250)) {
      this.requestPending = false;
      this.lastRequestAt = now;
      this.requestTiles();
    }
    if (missing > 0) this.invalidate(false);
  }

  private drawStale(b: PageBox, x: number, y: number, stale: number): void {
    const r = this.scale / stale;
    const cols = Math.ceil(b.width / r / TILE);
    const rows = Math.ceil(b.height / r / TILE);
    // Only the part in the viewport.
    const r0 = Math.max(0, Math.floor(-y / r / TILE));
    const r1 = Math.min(rows - 1, Math.floor((this.height - y) / r / TILE));
    const c0 = Math.max(0, Math.floor(-x / r / TILE));
    const c1 = Math.min(cols - 1, Math.floor((this.width - x) / r / TILE));
    if (r1 - r0 > 64 || c1 - c0 > 64) return;
    for (let row = r0; row <= r1; row++) {
      for (let col = c0; col <= c1; col++) {
        const bmp = this.cache.get(this.key(b.page, col, row, stale));
        if (bmp) {
          this.ctx.drawImage(
            bmp,
            x + col * TILE * r,
            y + row * TILE * r,
            bmp.width * r,
            bmp.height * r,
          );
        }
      }
    }
  }

  private requestTiles(): void {
    const info = this.info;
    if (!info) return;
    this.counters.requests++;
    const visible = this.visiblePages();
    // Fallback bitmaps first: one request per scale (pages of one size share it).
    const fallbacks = new Map<
      number,
      { page: number; col: number; row: number; priority: string }[]
    >();
    for (const b of visible) {
      const s = f32(this.fallbackScale(b.page));
      if (this.cache.has(this.key(b.page, 0, 0, s))) continue;
      const list = fallbacks.get(s) ?? [];
      list.push({ page: b.page, col: 0, row: 0, priority: "visible" });
      fallbacks.set(s, list);
    }
    for (const [scale, tiles] of fallbacks) this.send(scale, tiles);
    const tiles = [];
    for (const b of visible) {
      for (const t of this.tilesOf(b, 0)) {
        if (!this.cache.has(this.key(b.page, t.col, t.row, this.scale)))
          tiles.push({ page: b.page, ...t, priority: "visible" });
      }
    }
    // Then a viewport's worth above and below.
    const l = this.layout;
    if (l) {
      const sy = this.scrollY.value;
      for (const b of l.pagesIn(sy - this.height, sy + 2 * this.height)) {
        for (const t of this.tilesOf(b, this.height)) {
          if (!this.cache.has(this.key(b.page, t.col, t.row, this.scale)))
            tiles.push({ page: b.page, ...t, priority: "near" });
        }
      }
    }
    const seen = new Set<string>();
    const unique = tiles.filter((t) => {
      const k = `${t.page}:${t.col}:${t.row}`;
      if (seen.has(k)) return false;
      seen.add(k);
      return true;
    });
    this.send(f32(this.scale), unique);
  }

  private send(scale: number, tiles: unknown[]): void {
    const info = this.info;
    if (!info) return;
    void invoke("pdf_request_tiles", {
      doc: info.doc,
      scale,
      rotation: this.rotation,
      size: TILE,
      tiles,
    }).catch((err: unknown) => {
      console.error("pdf_request_tiles failed", err);
    });
  }

  private reportMemory(): void {
    if (this.reportTimer !== 0) return;
    this.reportTimer = window.setTimeout(() => {
      this.reportTimer = 0;
      const info = this.info;
      if (!info || this.cache.bytes === this.reportedBytes) return;
      this.reportedBytes = this.cache.bytes;
      void invoke("memory_report", {
        cache: "tile-bitmaps",
        key: info.doc,
        doc: null,
        bytes: this.cache.bytes,
      });
    }, 250);
  }

  private updateThumb(): void {
    const track = this.vtrack.clientHeight;
    const { top, length } = this.scrollY.thumb(track);
    this.vthumb.style.transform = `translateY(${top}px)`;
    this.vthumb.style.height = `${length}px`;
  }

  private scrollBy(dx: number, dy: number): void {
    const x = this.clampX(this.scrollX + dx);
    const moved = this.scrollY.scrollBy(dy) || x !== this.scrollX;
    this.scrollX = x;
    if (this.mode === "single" && dy !== 0 && !moved) {
      // At the page's end: on to the next or previous page.
      const next = this.single + Math.sign(dy);
      if (next >= 0 && next < (this.info?.pages ?? 0)) {
        this.gotoPage(next);
        if (dy < 0) this.scrollY.scrollTo(this.scrollY.max);
      }
      return;
    }
    if (moved) {
      this.invalidate(true);
      this.emit();
    }
  }

  private installInput(): void {
    this.element.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const rect = this.canvas.getBoundingClientRect();
        if (e.ctrlKey) {
          const anchor = {
            x: (e.clientX - rect.left) * this.dpr,
            y: (e.clientY - rect.top) * this.dpr,
          };
          this.zoomBy(Math.exp(-e.deltaY * (e.deltaMode === 0 ? 0.002 : 0.1)), anchor);
          return;
        }
        const unit =
          e.deltaMode === WheelEvent.DOM_DELTA_LINE
            ? LINE_CSS_PX * this.dpr
            : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
              ? this.height * 0.9
              : this.dpr;
        if (e.shiftKey) this.scrollBy(e.deltaY * unit, 0);
        else this.scrollBy(e.deltaX * unit, e.deltaY * unit);
      },
      { passive: false },
    );
    this.element.addEventListener("keydown", (e) => {
      const page = this.height * 0.9;
      const line = LINE_CSS_PX * this.dpr;
      const mod = e.ctrlKey || e.metaKey;
      const actions: Record<string, () => void> = {
        ArrowDown: () => {
          this.scrollBy(0, line);
        },
        ArrowUp: () => {
          this.scrollBy(0, -line);
        },
        ArrowRight: () => {
          if (this.layout && this.layout.width > this.width) this.scrollBy(line, 0);
          else this.gotoPage(this.currentPage() + 1);
        },
        ArrowLeft: () => {
          if (this.layout && this.layout.width > this.width) this.scrollBy(-line, 0);
          else this.gotoPage(this.currentPage() - 1);
        },
        PageDown: () => {
          this.scrollBy(0, page);
        },
        PageUp: () => {
          this.scrollBy(0, -page);
        },
        " ": () => {
          this.scrollBy(0, e.shiftKey ? -page : page);
        },
        Home: () => {
          this.gotoPage(0);
        },
        End: () => {
          this.gotoPage((this.info?.pages ?? 1) - 1);
        },
      };
      const zoomKeys: Record<string, () => void> = {
        "=": () => {
          this.zoomBy(1.25);
        },
        "+": () => {
          this.zoomBy(1.25);
        },
        "-": () => {
          this.zoomBy(0.8);
        },
        "0": () => {
          this.fit("fitWidth");
        },
      };
      const act = mod ? zoomKeys[e.key] : actions[e.key];
      if (act) {
        e.preventDefault();
        act();
      }
    });
    let grab: number | undefined;
    this.vthumb.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.vthumb.setPointerCapture(e.pointerId);
      grab = e.clientY - this.vthumb.getBoundingClientRect().top;
    });
    this.vthumb.addEventListener("pointermove", (e) => {
      if (grab === undefined) return;
      const top = e.clientY - this.vtrack.getBoundingClientRect().top - grab;
      if (this.scrollY.scrollTo(this.scrollY.offsetForThumb(top, this.vtrack.clientHeight))) {
        this.invalidate(true);
        this.emit();
      }
    });
    const release = () => {
      grab = undefined;
    };
    this.vthumb.addEventListener("pointerup", release);
    this.vthumb.addEventListener("pointercancel", release);
    this.vtrack.addEventListener("pointerdown", (e) => {
      const r = this.vthumb.getBoundingClientRect();
      this.scrollBy(0, e.clientY < r.top ? -this.height * 0.9 : this.height * 0.9);
    });
    // Drag to pan (a hand tool; text selection comes with task 7).
    let pan: { x: number; y: number } | undefined;
    this.canvas.addEventListener("pointerdown", (e) => {
      this.element.focus();
      if (e.button !== 0) return;
      this.canvas.setPointerCapture(e.pointerId);
      pan = { x: e.clientX, y: e.clientY };
    });
    this.canvas.addEventListener("pointermove", (e) => {
      if (!pan) return;
      this.scrollBy((pan.x - e.clientX) * this.dpr, (pan.y - e.clientY) * this.dpr);
      pan = { x: e.clientX, y: e.clientY };
    });
    const endPan = () => {
      pan = undefined;
    };
    this.canvas.addEventListener("pointerup", endPan);
    this.canvas.addEventListener("pointercancel", endPan);
  }
}
