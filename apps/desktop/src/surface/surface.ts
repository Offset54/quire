// Document surface compositor (MASTER_PROMPT §5.4; ADR-0004, ADR-0005).
//
// Imperative and outside React's render cycle. Two canvases:
// - tile layer: core-rendered tiles drawn at the scroll offset, with a
//   placeholder wherever a tile hasn't arrived (never a blank screen);
// - overlay layer: caret, selection and similar, from core geometry. Drawing
//   overlays never touches tiles.
// Scrolling is a logical offset in device px (ScrollModel) driven by wheel,
// keyboard and a custom scrollbar; nothing scrolls natively.

import { Channel, invoke } from "@tauri-apps/api/core";
import { tileKey, type TileId } from "./protocol";
import { ScrollModel } from "./scroll-model";
import { neededTiles, visibleTiles, wantedRange } from "./tile-grid";
import type { DecodedTile } from "./decode-worker";

export interface ViewInfo {
  view: number;
  layout: number;
  extent: { width: number; height: number; exact: boolean };
  transport: string;
}

export interface FrameSample {
  /** rAF timestamp. */
  time: number;
  /** Main-thread time spent composing this frame, ms. */
  workMs: number;
  visibleTiles: number;
  /** Visible tiles drawn as placeholders. */
  missingTiles: number;
  offset: number;
}

/** Tile delivery counters, for the HUD and benchmarks. */
export interface TileCounters {
  received: number;
  /** Arrived after the viewport had moved on; dropped without decoding. */
  droppedUnwanted: number;
  requests: number;
}

export type OverlayPainter = (ctx: CanvasRenderingContext2D, offset: number) => void;

const PLACEHOLDER = "#eceef3";
const LINE_CSS_PX = 40;
const SCROLLBAR_CSS_PX = 12;

interface SharedBufferEvent {
  getBuffer(): ArrayBuffer;
  additionalData: Partial<TileId> & { quireTile?: number; width?: number; height?: number };
}

interface WebView2 {
  addEventListener(type: "sharedbufferreceived", listener: (e: SharedBufferEvent) => void): void;
  releaseBuffer(buffer: ArrayBuffer): void;
}

const webview2 = (window as unknown as { chrome?: { webview?: WebView2 } }).chrome?.webview;
const surfacesByView = new Map<number, DocumentSurface>();
let sharedListenerInstalled = false;

/** One listener for all surfaces; routes shared buffers by view id. */
function installSharedBufferListener(): void {
  if (sharedListenerInstalled || !webview2) return;
  sharedListenerInstalled = true;
  webview2.addEventListener("sharedbufferreceived", (e) => {
    const d = e.additionalData;
    const buffer = e.getBuffer();
    const surface =
      d.quireTile === 1 && d.view !== undefined ? surfacesByView.get(d.view) : undefined;
    if (!surface || d.width === undefined || d.height === undefined) {
      webview2.releaseBuffer(buffer);
      return;
    }
    const id = d as TileId;
    if (!surface.wants(id)) {
      // Scrolled past already: skip the bitmap work entirely.
      surface.counters.droppedUnwanted++;
      webview2.releaseBuffer(buffer);
      return;
    }
    const pixels = new Uint8ClampedArray(buffer, 0, d.width * d.height * 4);
    createImageBitmap(new ImageData(pixels, d.width, d.height))
      .then((bitmap) => {
        surface.receive(id, bitmap);
      })
      .catch((err: unknown) => {
        console.error("tile bitmap failed", err);
      })
      .finally(() => {
        webview2.releaseBuffer(buffer);
      });
  });
}

/** LRU cache of tile bitmaps for one surface. */
class TileCache {
  private readonly tiles = new Map<string, ImageBitmap>();
  constructor(public capacity: number) {}

  get(key: string): ImageBitmap | undefined {
    const bitmap = this.tiles.get(key);
    if (bitmap) {
      // Refresh recency: Map iteration order is insertion order.
      this.tiles.delete(key);
      this.tiles.set(key, bitmap);
    }
    return bitmap;
  }

  has(key: string): boolean {
    return this.tiles.has(key);
  }

  put(key: string, bitmap: ImageBitmap): void {
    this.tiles.get(key)?.close();
    this.tiles.delete(key);
    this.tiles.set(key, bitmap);
    while (this.tiles.size > this.capacity) {
      const [oldest, b] = this.tiles.entries().next().value!;
      b.close();
      this.tiles.delete(oldest);
    }
  }

  clear(): void {
    for (const b of this.tiles.values()) b.close();
    this.tiles.clear();
  }

  get size(): number {
    return this.tiles.size;
  }
}

export class DocumentSurface {
  readonly element: HTMLDivElement;
  private readonly tileCanvas: HTMLCanvasElement;
  private readonly overlayCanvas: HTMLCanvasElement;
  private readonly tileCtx: CanvasRenderingContext2D;
  private readonly overlayCtx: CanvasRenderingContext2D;
  private readonly track: HTMLDivElement;
  private readonly thumb: HTMLDivElement;
  private readonly worker: Worker;
  private readonly cache = new TileCache(64);
  private readonly frameListeners = new Set<(s: FrameSample) => void>();
  private readonly scroll = new ScrollModel(0, 0);
  readonly counters: TileCounters = { received: 0, droppedUnwanted: 0, requests: 0 };
  /** Offset when tiles were last requested; a large jump since then means fast scrolling. */
  private lastRequestOffset = 0;

  private info: ViewInfo | undefined;
  private dpr = window.devicePixelRatio;
  private width = 0;
  private height = 0;
  private dirty = true;
  private requestPending = true;
  private lastRequestAt = 0;
  private lastOffset = 0;
  private direction: -1 | 0 | 1 = 0;
  private frameHandle = 0;
  private resizeObserver: ResizeObserver;
  overlay: OverlayPainter | undefined;

  constructor(
    host: HTMLElement,
    readonly tileSize = 512,
  ) {
    this.element = document.createElement("div");
    this.element.className = "surface";
    this.element.tabIndex = 0;
    this.tileCanvas = document.createElement("canvas");
    this.overlayCanvas = document.createElement("canvas");
    this.overlayCanvas.className = "surface-overlay";
    this.track = document.createElement("div");
    this.track.className = "surface-scrollbar";
    this.thumb = document.createElement("div");
    this.thumb.className = "surface-thumb";
    this.track.append(this.thumb);
    this.element.append(this.tileCanvas, this.overlayCanvas, this.track);
    host.append(this.element);

    this.tileCtx = this.tileCanvas.getContext("2d", { alpha: false })!;
    this.overlayCtx = this.overlayCanvas.getContext("2d")!;

    this.worker = new Worker(new URL("./decode-worker.ts", import.meta.url), { type: "module" });
    this.worker.onmessage = (e: MessageEvent<DecodedTile>) => {
      this.receive(e.data.id, e.data.bitmap);
    };

    this.installInput();
    this.resizeObserver = new ResizeObserver(() => {
      this.resize();
    });
    this.resizeObserver.observe(this.element);
    this.resize();
    installSharedBufferListener();
  }

  /** A Channel for the command that opens this surface's view (fallback transport). */
  createChannel(): Channel<ArrayBuffer> {
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buf) => {
      this.worker.postMessage(buf, [buf]);
    };
    return channel;
  }

  attach(info: ViewInfo): void {
    if (this.info) surfacesByView.delete(this.info.view);
    this.info = info;
    surfacesByView.set(info.view, this);
    this.scroll.setExtent(info.extent.height);
    this.cache.clear();
    this.invalidate(true);
  }

  get view(): ViewInfo | undefined {
    return this.info;
  }

  get offset(): number {
    return this.scroll.value;
  }

  get maxOffset(): number {
    return this.scroll.max;
  }

  /** Device-px size of the tile layer. */
  get viewport(): { width: number; height: number; dpr: number } {
    return { width: this.width, height: this.height, dpr: this.dpr };
  }

  scrollTo(offset: number): void {
    if (this.scroll.scrollTo(offset)) this.invalidate(true);
  }

  scrollBy(delta: number): void {
    if (this.scroll.scrollBy(delta)) this.invalidate(true);
  }

  onFrame(listener: (s: FrameSample) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

  /** Number of visible tiles not yet drawn sharp. */
  missingVisible(): number {
    const info = this.info;
    if (!info) return 0;
    return visibleTiles(this.grid(info)).filter(
      (t) => !this.cache.has(this.key(info, t.col, t.row)),
    ).length;
  }

  /**
   * Whether a tile is still worth decoding: current layout, and inside the
   * range the surface could currently be asking for.
   */
  wants(id: TileId): boolean {
    const info = this.info;
    if (!info || id.view !== info.view || id.layout !== info.layout || id.size !== this.tileSize) {
      return false;
    }
    const top = id.row * this.tileSize;
    const [lo, hi] = wantedRange(this.scroll.value, this.height);
    return top + this.tileSize > lo && top < hi;
  }

  /** PNG data URL of the tile layer, for visual checks in benchmarks and tests. */
  snapshot(): string {
    return this.tileCanvas.toDataURL("image/png");
  }

  receive(id: TileId, bitmap: ImageBitmap): void {
    if (!this.wants(id)) {
      this.counters.droppedUnwanted++;
      bitmap.close();
      return;
    }
    this.counters.received++;
    this.cache.put(tileKey(id), bitmap);
    this.invalidate(false);
  }

  async close(): Promise<void> {
    cancelAnimationFrame(this.frameHandle);
    this.resizeObserver.disconnect();
    this.worker.terminate();
    this.cache.clear();
    if (this.info) {
      surfacesByView.delete(this.info.view);
      await invoke("view_close", { view: this.info.view });
    }
    this.element.remove();
  }

  // ---- internals -------------------------------------------------------------

  private key(info: ViewInfo, col: number, row: number): string {
    return tileKey({ view: info.view, layout: info.layout, size: this.tileSize, col, row });
  }

  private grid(info: ViewInfo) {
    return {
      offset: this.scroll.value,
      viewportWidth: this.width,
      viewportHeight: this.height,
      extentWidth: info.extent.width,
      extentHeight: info.extent.height,
      tileSize: this.tileSize,
      direction: this.direction,
    };
  }

  private invalidate(viewportChanged: boolean): void {
    this.dirty = true;
    if (viewportChanged) this.requestPending = true;
    if (this.frameHandle === 0) {
      this.frameHandle = requestAnimationFrame((t) => {
        this.frame(t);
      });
    }
  }

  private resize(): void {
    this.dpr = window.devicePixelRatio;
    const rect = this.element.getBoundingClientRect();
    const cssWidth = Math.max(0, rect.width - SCROLLBAR_CSS_PX);
    this.width = Math.round(cssWidth * this.dpr);
    this.height = Math.round(rect.height * this.dpr);
    for (const c of [this.tileCanvas, this.overlayCanvas]) {
      c.width = this.width;
      c.height = this.height;
      c.style.width = `${cssWidth}px`;
      c.style.height = `${rect.height}px`;
    }
    this.scroll.setViewport(this.height);
    const visible =
      Math.ceil(this.width / this.tileSize) * (Math.ceil(this.height / this.tileSize) + 1);
    this.cache.capacity = Math.max(64, visible * 10);
    const info = this.info;
    if (info && this.width > 0 && Math.round(info.extent.width) !== this.width) {
      void invoke<ViewInfo>("view_relayout", { view: info.view, width: this.width }).then(
        (next) => {
          this.attach(next);
        },
      );
    }
    this.invalidate(true);
  }

  private frame(time: number): void {
    this.frameHandle = 0;
    const info = this.info;
    if (!this.dirty || !info) return;
    this.dirty = false;
    const start = performance.now();
    const offset = this.scroll.value;
    this.direction = offset > this.lastOffset ? 1 : offset < this.lastOffset ? -1 : this.direction;
    this.lastOffset = offset;

    const ctx = this.tileCtx;
    const T = this.tileSize;
    const visible = visibleTiles(this.grid(info));
    let missing = 0;
    ctx.fillStyle = PLACEHOLDER;
    ctx.fillRect(0, 0, this.width, this.height);
    for (const { col, row } of visible) {
      const bitmap = this.cache.get(this.key(info, col, row));
      if (bitmap) ctx.drawImage(bitmap, col * T, row * T - offset);
      else missing++;
    }

    this.overlayCtx.clearRect(0, 0, this.width, this.height);
    this.overlay?.(this.overlayCtx, offset);
    this.updateThumb();

    // Ask for missing tiles when the viewport moved, or every 250 ms while
    // some are still missing (a dropped request must not leave holes).
    const now = performance.now();
    if (this.requestPending || (missing > 0 && now - this.lastRequestAt > 250)) {
      this.requestPending = false;
      this.lastRequestAt = now;
      this.requestTiles(info);
    }
    if (missing > 0) this.invalidate(false); // keep polling the cache while tiles stream in

    const sample: FrameSample = {
      time,
      workMs: performance.now() - start,
      visibleTiles: visible.length,
      missingTiles: missing,
      offset,
    };
    for (const l of this.frameListeners) l(sample);
  }

  private requestTiles(info: ViewInfo): void {
    // While the view moves more than a viewport between requests (thumb
    // drags, flings), anything beyond the visible tiles is obsolete before it
    // arrives, so only ask for what is on screen.
    const fast = Math.abs(this.scroll.value - this.lastRequestOffset) > this.height;
    this.lastRequestOffset = this.scroll.value;
    this.counters.requests++;
    const tiles = neededTiles(this.grid(info)).filter(
      (t) => (!fast || t.priority === "visible") && !this.cache.has(this.key(info, t.col, t.row)),
    );
    void invoke("view_request_tiles", {
      view: info.view,
      layout: info.layout,
      size: this.tileSize,
      tiles,
    }).catch((err: unknown) => {
      console.error("view_request_tiles failed", err);
    });
  }

  private updateThumb(): void {
    const track = this.track.clientHeight;
    const { top, length } = this.scroll.thumb(track);
    this.thumb.style.transform = `translateY(${top}px)`;
    this.thumb.style.height = `${length}px`;
  }

  private installInput(): void {
    this.element.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const unit =
          e.deltaMode === WheelEvent.DOM_DELTA_LINE
            ? LINE_CSS_PX * this.dpr
            : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
              ? this.height * 0.9
              : this.dpr;
        this.scrollBy(e.deltaY * unit);
      },
      { passive: false },
    );

    this.element.addEventListener("keydown", (e) => {
      const page = this.height * 0.9;
      const line = LINE_CSS_PX * this.dpr;
      const moves: Record<string, () => void> = {
        ArrowDown: () => {
          this.scrollBy(line);
        },
        ArrowUp: () => {
          this.scrollBy(-line);
        },
        PageDown: () => {
          this.scrollBy(page);
        },
        PageUp: () => {
          this.scrollBy(-page);
        },
        Home: () => {
          this.scrollTo(0);
        },
        End: () => {
          this.scrollTo(this.scroll.max);
        },
      };
      const move = moves[e.key];
      if (move) {
        e.preventDefault();
        move();
      }
    });

    // Thumb drag maps the thumb's position to an offset.
    let grab: number | undefined;
    this.thumb.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.thumb.setPointerCapture(e.pointerId);
      grab = e.clientY - this.thumb.getBoundingClientRect().top;
    });
    this.thumb.addEventListener("pointermove", (e) => {
      if (grab === undefined) return;
      const top = e.clientY - this.track.getBoundingClientRect().top - grab;
      this.scrollTo(this.scroll.offsetForThumb(top, this.track.clientHeight));
    });
    const release = () => {
      grab = undefined;
    };
    this.thumb.addEventListener("pointerup", release);
    this.thumb.addEventListener("pointercancel", release);

    // Clicking the track pages toward the click.
    this.track.addEventListener("pointerdown", (e) => {
      const thumbRect = this.thumb.getBoundingClientRect();
      this.scrollBy(e.clientY < thumbRect.top ? -this.height * 0.9 : this.height * 0.9);
    });
  }
}
