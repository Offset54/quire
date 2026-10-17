// Exports (M2 task 11, ADR-0031 and its addendum): self-contained HTML and
// PDF, built without long tasks on the page's main thread (RISKS #17).
//
// The core renders the whole document block by block on its own thread,
// code highlighted and images inlined (`md_export_begin`). The page takes
// the HTML in batches of whole blocks (`md_export_take`) and hands each to
// a worker, which renders the math with KaTeX and splits the batch at its
// diagrams; the page renders those in the Mermaid frame (ADR-0033) one at
// a time and puts every piece back to the host in order (`md_export_put`).
// The head (the export stylesheet and, when there is math, KaTeX's with
// its fonts as data URLs) goes last; the host keeps the page and writes it
// as HTML or prints it to PDF (`md_export_finish`). Nothing here parses or
// serializes the page. Everything not carried over is in the report.

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import exportCss from "./export.css?raw";
import type { WorkerRequest, WorkerResponse } from "./export-worker";
import { escapeHtml, type MathLabels } from "./math-core";
import { DiagramFrame } from "./rich";

export interface ReportItem {
  kind: string;
  source: string;
  reason: string;
}

/** A built page, kept by the host until it is finished or aborted. */
export interface Built {
  id: number;
  title: string;
  /** The page's size in bytes. */
  bytes: number;
  report: ReportItem[];
}

/** The error an export rejects with once cancelled. */
export class ExportCancelled extends Error {
  constructor() {
    super("cancelled");
  }
}

/** KaTeX's fonts as data URLs, loaded only when a document has math. */
const fontModules = import.meta.glob<string>("../../node_modules/katex/dist/fonts/*.woff2", {
  query: "?inline",
  import: "default",
});

async function katexCss(): Promise<string> {
  const css = (await import("katex/dist/katex.min.css?raw")).default;
  const fonts = new Map<string, string>();
  await Promise.all(
    Object.entries(fontModules).map(async ([path, load]) => {
      fonts.set(path.slice(path.lastIndexOf("/") + 1), await load());
    }),
  );
  // Each face: the woff2 source only, as a data URL.
  return css.replace(/src:\s*url\(fonts\/([^)]+\.woff2)\)[^;}]*/g, (m, name: string) => {
    const data = fonts.get(name);
    return data ? `src:url(${data}) format("woff2")` : m;
  });
}

/** The worker, one request at a time. */
class MathWorker {
  private readonly worker = new Worker(new URL("./export-worker.ts", import.meta.url), {
    type: "module",
  });
  private seq = 0;
  private readonly labels: MathLabels = {
    error: t("md.mathError"),
    tooLarge: t("md.mathTooLarge"),
  };

  process(html: ArrayBuffer): Promise<WorkerResponse> {
    const seq = ++this.seq;
    return new Promise((resolve, reject) => {
      this.worker.onmessage = (e: MessageEvent<WorkerResponse>) => {
        if (e.data.seq === seq) resolve(e.data);
      };
      this.worker.onerror = (e) => {
        reject(new Error(e.message || "the export worker failed"));
      };
      const req: WorkerRequest = { seq, labels: this.labels, html };
      this.worker.postMessage(req, [html]);
    });
  }

  destroy(): void {
    this.worker.terminate();
  }
}

const encoder = new TextEncoder();

function put(id: number, piece: "head" | "body" | "tail", bytes: ArrayBuffer | Uint8Array) {
  return invoke("md_export_put", bytes, {
    headers: { "x-quire-export": String(id), "x-quire-piece": piece },
  });
}

/** A diagram's SVG as an <img> (as `diagramFigure` makes it). */
function figureHtml(svg: string): string {
  return (
    `<figure class="md-diagram"><img alt="${escapeHtml(t("md.diagramAlt"))}" decoding="async" ` +
    `src="data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}"></figure>`
  );
}

/** Builds the export page of document `doc` in the host. `progress` gets
 *  the fraction of the source done; `signal` cancels (the job is aborted
 *  and this rejects with `ExportCancelled`). */
export async function buildExport(
  doc: number,
  frameUrl: string,
  progress: (fraction: number) => void,
  signal: AbortSignal,
): Promise<Built> {
  const job = await invoke<{ id: number; title: string; total: number }>("md_export_begin", {
    doc,
  });
  const id = job.id;
  const report: ReportItem[] = [];
  const worker = new MathWorker();
  let frame: DiagramFrame | undefined;
  const check = () => {
    if (signal.aborted) throw new ExportCancelled();
  };
  const take = () => invoke<ArrayBuffer>("md_export_take", { id });
  try {
    let math = 0;
    // The next batch is asked for while this one is processed.
    let next = take();
    for (;;) {
      const batch = await next;
      check();
      if (batch.byteLength <= 8) break;
      const reached = Number(new DataView(batch).getBigUint64(0, true));
      next = take();
      // Keep a rejection of the prefetch from going unhandled when this
      // batch fails first.
      next.catch(() => undefined);
      const r = await worker.process(batch.slice(8));
      check();
      math += r.math;
      report.push(...r.report);
      for (const p of r.pieces) {
        if (p instanceof ArrayBuffer) {
          if (p.byteLength > 0) await put(id, "body", p);
          continue;
        }
        frame ??= new DiagramFrame(frameUrl);
        const d = await frame.render(p.diagram);
        check();
        if ("html" in d) {
          await put(id, "body", encoder.encode(figureHtml(d.html)));
        } else {
          report.push({ kind: "diagram", source: p.diagram.slice(0, 120), reason: d.error });
          // The source stays in the page.
          await put(
            id,
            "body",
            encoder.encode(
              `<pre data-lang="mermaid"><code class="language-mermaid">${escapeHtml(p.diagram)}</code></pre>`,
            ),
          );
        }
      }
      progress(job.total > 0 ? reached / job.total : 1);
    }
    const css = exportCss + (math > 0 ? `\n${await katexCss()}` : "");
    check();
    const head =
      `<!doctype html>\n<html>\n<head>\n<meta charset="utf-8">\n` +
      `<meta name="viewport" content="width=device-width, initial-scale=1">\n` +
      `<meta name="generator" content="quire">\n` +
      `<title>${escapeHtml(job.title)}</title>\n<style>\n${css}\n</style>\n</head>\n<body>\n` +
      `<article class="markdown-body">`;
    await put(id, "head", encoder.encode(head));
    await put(id, "tail", encoder.encode(`</article>\n</body>\n</html>\n`));
    const sealed = await invoke<{ bytes: number; report: ReportItem[] }>("md_export_seal", { id });
    return { id, title: job.title, bytes: sealed.bytes, report: [...sealed.report, ...report] };
  } catch (err) {
    void abortExport(id);
    if (signal.aborted) throw new ExportCancelled();
    throw err;
  } finally {
    worker.destroy();
    frame?.destroy();
  }
}

/** Drops a built page (or stops one being built or printed). */
export function abortExport(id: number): Promise<void> {
  return invoke("md_export_abort", { id });
}

/** Writes a built page to `path` as HTML, or prints it there as PDF;
 *  `signal` cancels (a PDF print is abandoned, nothing is written). */
export async function finishExport(
  built: Built,
  path: string,
  kind: "html" | "pdf",
  signal: AbortSignal,
): Promise<void> {
  const onAbort = () => {
    void abortExport(built.id);
  };
  signal.addEventListener("abort", onAbort);
  try {
    await invoke("md_export_finish", { id: built.id, path, kind });
  } catch (err) {
    if (signal.aborted) throw new ExportCancelled();
    throw err;
  } finally {
    signal.removeEventListener("abort", onAbort);
  }
}

/** Largest page printed to PDF (user decision 2026-10-16, ADR-0031): the
 *  webview's printer needs gigabytes beyond this (a 29 MB page grew it past
 *  5.8 GB); 10 MiB is about 1 MiB of math-heavy Markdown, ~1,100 pages.
 *  The host checks the same limit (`md.rs`). */
export const PDF_MAX_BYTES = 10 * 1024 * 1024;

/** Whether a page of `bytes` is too large to print to PDF. */
export function pdfTooLarge(bytes: number): boolean {
  return bytes > PDF_MAX_BYTES;
}
