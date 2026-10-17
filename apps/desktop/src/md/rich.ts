// Math and diagrams in the preview (ADR-0033), rendered lazily for blocks
// near the viewport, after the paint (§7: progressive everything):
//
// - Math (`data-math-style`, from comrak): KaTeX, loaded on first use, in
//   slices of at most SLICE_MS per task (§2: no UI-thread task over 8 ms),
//   cached by source. Until then the TeX source shows, labeled pending; on
//   an error, the source with an error badge.
// - Mermaid (```mermaid): rendered in a sandboxed iframe served by the
//   host's `quire-frame` scheme, one diagram at a time; the SVG comes back
//   as text and is shown as an <img> (a data: URL), so nothing in it runs
//   or loads in this document. Cached by source and theme.
//
// Both caches report their size to the memory governor ("md-rich").

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "../i18n";
import type { FrameRequest } from "../mermaid-frame";
import { MAX_MATH, katexOptions } from "./math-core";

const SLICE_MS = 4;
/** Sources larger than this aren't rendered (labeled). */
const MAX_DIAGRAM = 50 * 1024;
const DIAGRAM_TIMEOUT_MS = 10_000;
/** Entries kept per cache (least recently used out). */
const CACHE_ENTRIES = 2000;

type Katex = typeof import("katex").default;

export type Result = { html: string } | { error: string };

class Lru<V> {
  private readonly map = new Map<string, V>();
  bytes = 0;
  constructor(
    private readonly max: number,
    private readonly size: (v: V) => number,
  ) {}
  get(k: string): V | undefined {
    const v = this.map.get(k);
    if (v !== undefined) {
      this.map.delete(k);
      this.map.set(k, v);
    }
    return v;
  }
  set(k: string, v: V): void {
    const old = this.map.get(k);
    if (old !== undefined) this.bytes -= this.size(old) + k.length * 2;
    this.map.delete(k);
    this.map.set(k, v);
    this.bytes += this.size(v) + k.length * 2;
    while (this.map.size > this.max) {
      const [ok, ov] = this.map.entries().next().value as [string, V];
      this.map.delete(ok);
      this.bytes -= this.size(ov) + ok.length * 2;
    }
  }
  clear(): void {
    this.map.clear();
    this.bytes = 0;
  }
}

const resultSize = (r: Result) => ("html" in r ? r.html.length : r.error.length) * 2;
const mathCache = new Lru<Result>(CACHE_ENTRIES, resultSize);
const diagramCache = new Lru<Result>(CACHE_ENTRIES / 4, resultSize);

let katexLoad: Promise<Katex> | undefined;
export function loadKatex(): Promise<Katex> {
  katexLoad ??= import("./katex").then((m) => m.default);
  return katexLoad;
}

function dark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function badge(el: HTMLElement, text: string, title?: string): void {
  const b = el.ownerDocument.createElement("span");
  b.className = "md-rich-badge";
  b.textContent = text;
  if (title) b.title = title;
  el.prepend(b);
}

/** The Mermaid frame: created on first use, kept for the session. */
export class DiagramFrame {
  private frame: HTMLIFrameElement | undefined;
  private ready: Promise<void> | undefined;
  private next = 1;
  private readonly waiting = new Map<number, (r: Result) => void>();

  constructor(private readonly url: string) {}

  private start(): Promise<void> {
    if (this.ready) return this.ready;
    const f = document.createElement("iframe");
    // Scripts only: no same origin (an opaque origin, no IPC, no storage),
    // no top navigation, no popups, no forms.
    f.setAttribute("sandbox", "allow-scripts");
    f.setAttribute("aria-hidden", "true");
    f.tabIndex = -1;
    f.className = "md-diagram-frame";
    f.src = this.url;
    this.frame = f;
    this.ready = new Promise<void>((resolve, reject) => {
      // The frame's messages are untrusted data: checked field by field.
      window.addEventListener("message", (e: MessageEvent<unknown>) => {
        if (e.source !== f.contentWindow) return;
        if (typeof e.data !== "object" || e.data === null) return;
        const d = e.data as Partial<Record<"ready" | "failed" | "id" | "svg" | "error", unknown>>;
        if (d.ready === true) {
          resolve();
          return;
        }
        if (typeof d.failed === "string") {
          reject(new Error(d.failed));
          return;
        }
        if (typeof d.id !== "number") return;
        const done = this.waiting.get(d.id);
        if (!done) return;
        this.waiting.delete(d.id);
        done(
          typeof d.svg === "string"
            ? { html: d.svg }
            : { error: typeof d.error === "string" ? d.error : "?" },
        );
      });
    });
    document.body.append(f);
    return this.ready;
  }

  async render(source: string): Promise<Result> {
    let timer = 0;
    const started = await Promise.race([
      this.start().then(
        () => "",
        (e: unknown) => (e instanceof Error ? e.message : String(e)),
      ),
      new Promise<string>((resolve) => {
        timer = window.setTimeout(() => {
          resolve(t("md.diagramFrameFailed"));
        }, DIAGRAM_TIMEOUT_MS);
      }),
    ]);
    window.clearTimeout(timer);
    if (started !== "") return { error: started };
    const id = this.next++;
    return new Promise<Result>((resolve) => {
      const timer = window.setTimeout(() => {
        this.waiting.delete(id);
        resolve({ error: t("md.diagramTimeout") });
      }, DIAGRAM_TIMEOUT_MS);
      this.waiting.set(id, (r) => {
        window.clearTimeout(timer);
        resolve(r);
      });
      const req: FrameRequest = { id, source, dark: dark() };
      this.frame?.contentWindow?.postMessage(req, "*");
    });
  }

  destroy(): void {
    this.frame?.remove();
  }
}

/** Renders one `data-math-style` element in place (the preview; exports
 *  use `renderMathHtml`, its string twin); returns the error, if any. */
export function renderMathElement(katex: Katex, el: HTMLElement): string | undefined {
  const doc = el.ownerDocument;
  const display = el.dataset.mathStyle === "display";
  const src = el.textContent;
  el.dataset.rich = "done";
  if (src.length > MAX_MATH) {
    el.classList.add("md-rich-plain");
    badge(el, t("md.mathTooLarge"));
    return t("md.mathTooLarge");
  }
  const key = `${display ? "D" : "I"}${src}`;
  let r = mathCache.get(key);
  if (!r) {
    try {
      r = { html: katex.renderToString(src, katexOptions(display)) };
    } catch (e) {
      r = { error: e instanceof Error ? e.message : String(e) };
    }
    mathCache.set(key, r);
  }
  if ("error" in r) {
    el.classList.add("md-rich-error");
    badge(el, t("md.mathError"), r.error);
    return r.error;
  }
  // Display math in a code block (```math): replace the <pre>.
  const target =
    el.tagName === "CODE" && el.parentElement?.tagName === "PRE" ? el.parentElement : el;
  const out = doc.createElement(display ? "div" : "span");
  out.className = display ? "md-math md-math-display" : "md-math";
  const pos = target.getAttribute("data-sourcepos");
  if (pos) out.setAttribute("data-sourcepos", pos);
  out.dataset.rich = "done";
  out.innerHTML = r.html;
  target.replaceWith(out);
  return undefined;
}

/** A diagram's SVG as an <img> (preview and exports). */
export function diagramFigure(doc: Document, svg: string, sourcepos: string | null): HTMLElement {
  const fig = doc.createElement("figure");
  fig.className = "md-diagram";
  if (sourcepos) fig.setAttribute("data-sourcepos", sourcepos);
  const img = doc.createElement("img");
  img.alt = t("md.diagramAlt");
  img.decoding = "async";
  img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  fig.append(img);
  return fig;
}

let trimListenerInstalled = false;

/** The governor asks for the caches back (frontend_cache.rs). */
function installTrimListener(): void {
  if (trimListenerInstalled) return;
  trimListenerInstalled = true;
  void listen<{ cache: string }>("memory-trim", (e) => {
    if (e.payload.cache !== "md-rich") return;
    mathCache.clear();
    diagramCache.clear();
    void invoke("memory_report", { cache: "md-rich", key: 0, doc: null, bytes: 0 });
  });
}

export class RichRenderer {
  private readonly math: HTMLElement[] = [];
  private readonly diagrams: HTMLElement[] = [];
  private mathTimer = 0;
  private diagramBusy = false;
  private readonly frame: DiagramFrame;
  private destroyed = false;
  private reportTimer = 0;

  constructor(
    frameUrl: string,
    /** A block's height may have changed. */
    private readonly changed: () => void,
  ) {
    this.frame = new DiagramFrame(frameUrl);
    installTrimListener();
  }

  /** Queues the math and diagrams of a placed block near the viewport
   *  (once per rendering: the block's `data-rich-seen` is cleared when its
   *  HTML is replaced). */
  visit(block: HTMLElement): void {
    if (this.destroyed || block.dataset.richSeen === "1") return;
    block.dataset.richSeen = "1";
    for (const el of block.querySelectorAll<HTMLElement>("[data-math-style]:not([data-rich])")) {
      el.dataset.rich = "queued";
      this.math.push(el);
    }
    for (const el of block.querySelectorAll<HTMLElement>(
      'pre[data-lang="mermaid"]:not([data-rich])',
    )) {
      el.dataset.rich = "queued";
      this.diagrams.push(el);
    }
    if (this.math.length > 0) this.scheduleMath();
    if (this.diagrams.length > 0) void this.pumpDiagrams();
  }

  private scheduleMath(): void {
    if (this.mathTimer !== 0) return;
    this.mathTimer = window.setTimeout(() => {
      void this.mathSlice();
    }, 0);
  }

  private async mathSlice(): Promise<void> {
    const katex = await loadKatex();
    this.mathTimer = 0;
    if (this.destroyed) return;
    const t0 = performance.now();
    let n = 0;
    while (this.math.length > 0 && performance.now() - t0 < SLICE_MS) {
      const el = this.math.shift()!;
      if (!el.isConnected) continue;
      this.renderMath(katex, el);
      n++;
    }
    if (n > 0) {
      this.changed();
      this.report();
    }
    if (this.math.length > 0) this.scheduleMath();
  }

  private renderMath(katex: Katex, el: HTMLElement): void {
    renderMathElement(katex, el);
  }

  private async pumpDiagrams(): Promise<void> {
    if (this.diagramBusy) return;
    this.diagramBusy = true;
    try {
      while (this.diagrams.length > 0 && !this.destroyed) {
        const pre = this.diagrams.shift()!;
        if (!pre.isConnected) continue;
        await this.renderDiagram(pre);
      }
    } finally {
      this.diagramBusy = false;
    }
  }

  private async renderDiagram(pre: HTMLElement): Promise<void> {
    const src = pre.querySelector("code")?.textContent ?? "";
    if (src.length > MAX_DIAGRAM) {
      pre.dataset.rich = "done";
      badge(pre, t("md.diagramTooLarge"));
      this.changed();
      return;
    }
    pre.dataset.rich = "pending";
    const key = `${dark() ? "D" : "L"}${src}`;
    let r = diagramCache.get(key);
    if (!r) {
      r = await this.frame.render(src);
      diagramCache.set(key, r);
      this.report();
    }
    if (!pre.isConnected) return;
    pre.dataset.rich = "done";
    if ("error" in r) {
      pre.classList.add("md-rich-error");
      badge(pre, t("md.diagramError"), r.error);
      this.changed();
      return;
    }
    const fig = diagramFigure(document, r.html, pre.getAttribute("data-sourcepos"));
    const img = fig.querySelector("img");
    if (img) {
      img.onload = () => {
        this.changed();
      };
    }
    pre.replaceWith(fig);
    this.changed();
  }

  private report(): void {
    if (this.reportTimer !== 0) return;
    this.reportTimer = window.setTimeout(() => {
      this.reportTimer = 0;
      if (this.destroyed) return;
      void invoke("memory_report", {
        cache: "md-rich",
        key: 0,
        // Shared by every document.
        doc: null,
        bytes: mathCache.bytes + diagramCache.bytes,
      });
    }, 500);
  }

  destroy(): void {
    this.destroyed = true;
    this.math.length = 0;
    this.diagrams.length = 0;
    window.clearTimeout(this.mathTimer);
    window.clearTimeout(this.reportTimer);
    this.frame.destroy();
  }
}
