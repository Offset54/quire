// The export's math, off the page's main thread (ADR-0031 addendum,
// RISKS #17): each batch of the core's HTML comes in as UTF-8 bytes; math
// is rendered with KaTeX (math-core.ts) and the batch goes back as UTF-8
// bytes, split at its Mermaid blocks, whose sources the page renders in
// the diagram frame (it needs the DOM).

import katex from "katex";
import { renderMathHtml, splitDiagrams, type MathLabels, type MathReport } from "./math-core";

export interface WorkerRequest {
  seq: number;
  labels: MathLabels;
  /** UTF-8 HTML (transferred). */
  html: ArrayBuffer;
}

export interface WorkerResponse {
  seq: number;
  /** UTF-8 HTML pieces (transferred) and diagram sources, in order. */
  pieces: (ArrayBuffer | { diagram: string })[];
  report: MathReport[];
  /** Math expressions in the batch. */
  math: number;
}

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<WorkerRequest>) => void) | null;
  postMessage(msg: WorkerResponse, transfer: Transferable[]): void;
};
const cache = new Map<string, { html: string } | { error: string }>();
const decoder = new TextDecoder();
const encoder = new TextEncoder();

scope.onmessage = (e) => {
  const { seq, labels, html } = e.data;
  const report: MathReport[] = [];
  const r = renderMathHtml(decoder.decode(html), katex, labels, cache, report);
  const transfer: ArrayBuffer[] = [];
  const pieces = splitDiagrams(r.html).map((p) => {
    if (typeof p !== "string") return p;
    const bytes = encoder.encode(p).buffer;
    transfer.push(bytes);
    return bytes;
  });
  scope.postMessage({ seq, pieces, report, math: r.math }, transfer);
};
