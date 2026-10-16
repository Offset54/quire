// Exports (M2 task 11, ADR-0031): self-contained HTML and PDF.
//
// The core renders the whole document with code highlighted and images
// inlined (`md_export_html`). Here math is rendered with KaTeX and diagrams
// in the Mermaid frame (ADR-0033), the export stylesheet and, when there is
// math, KaTeX's stylesheet with its fonts as data URLs are added, and the
// page is written as HTML or printed to PDF by the host (`md_export_save`).
// Everything not carried over is in the returned report.

import { invoke } from "@tauri-apps/api/core";
import exportCss from "./export.css?raw";
import { DiagramFrame, diagramFigure, loadKatex, renderMathElement } from "./rich";

export interface ReportItem {
  kind: string;
  source: string;
  reason: string;
}

export interface Exported {
  /** The finished page. */
  html: string;
  title: string;
  report: ReportItem[];
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

const escapeHtml = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** Builds the export page of document `doc`. */
export async function buildExport(doc: number, frameUrl: string): Promise<Exported> {
  const r = await invoke<{ html: string; report: ReportItem[]; title: string }>("md_export_html", {
    doc,
  });
  const report = [...r.report];
  // An inert document: nothing in it runs or loads.
  const page = new DOMParser().parseFromString(
    `<!doctype html><html><body><article class="markdown-body">${r.html}</article></body></html>`,
    "text/html",
  );
  const math = [...page.querySelectorAll<HTMLElement>("[data-math-style]")];
  if (math.length > 0) {
    const katex = await loadKatex();
    for (const el of math) {
      const src = el.textContent;
      const err = renderMathElement(katex, el);
      if (err) report.push({ kind: "math", source: src.slice(0, 120), reason: err });
    }
  }
  const diagrams = [...page.querySelectorAll<HTMLElement>('pre[data-lang="mermaid"]')];
  if (diagrams.length > 0) {
    const frame = new DiagramFrame(frameUrl);
    try {
      for (const pre of diagrams) {
        const src = pre.querySelector("code")?.textContent ?? "";
        const d = await frame.render(src);
        if ("html" in d) pre.replaceWith(diagramFigure(page, d.html, null));
        else report.push({ kind: "diagram", source: src.slice(0, 120), reason: d.error });
      }
    } finally {
      frame.destroy();
    }
  }
  // Sources positions are for the preview only.
  for (const el of page.querySelectorAll("[data-sourcepos]")) el.removeAttribute("data-sourcepos");
  const css = exportCss + (math.length > 0 ? `\n${await katexCss()}` : "");
  const body = page.querySelector("article")?.outerHTML ?? "";
  const html =
    `<!doctype html>\n<html>\n<head>\n<meta charset="utf-8">\n` +
    `<meta name="viewport" content="width=device-width, initial-scale=1">\n` +
    `<meta name="generator" content="quire">\n` +
    `<title>${escapeHtml(r.title)}</title>\n<style>\n${css}\n</style>\n</head>\n<body>\n${body}\n</body>\n</html>\n`;
  return { html, title: r.title, report };
}

/** Largest page printed to PDF (user decision 2026-10-16, ADR-0031): the
 *  webview's printer needs gigabytes beyond this (a 29 MB page grew it past
 *  5.8 GB); 10 MiB is about 1 MiB of math-heavy Markdown, ~1,100 pages.
 *  The host checks the same limit (`md.rs`). */
export const PDF_MAX_BYTES = 10 * 1024 * 1024;

/** The page's size in bytes when it is too large to print to PDF. */
export function pdfTooLarge(html: string): number | null {
  const bytes = new TextEncoder().encode(html).length;
  return bytes > PDF_MAX_BYTES ? bytes : null;
}

/** Writes the page to `path` as HTML, or prints it there as PDF. */
export async function saveExport(html: string, path: string, kind: "html" | "pdf"): Promise<void> {
  await invoke("md_export_save", new TextEncoder().encode(html), {
    headers: { "x-quire-path": encodeURIComponent(path), "x-quire-kind": kind },
  });
}
