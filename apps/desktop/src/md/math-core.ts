// Math and diagrams in an export's HTML as strings (ADR-0031 addendum,
// RISKS #17): the export renders math in a worker, which has no DOM, so
// it works on the core's markup directly. The core writes math in exactly
// two forms (comrak's; raw HTML can't make them, the sanitizer drops
// `data-math-style`) and Mermaid blocks in one, with the source escaped as
// text in each:
//
//   <span data-math-style="inline|display">TEX</span>
//   <pre><code class="language-math" data-math-style="display">TEX</code></pre>
//   <pre data-lang="mermaid" data-hl="none"><code class="language-mermaid">SRC</code></pre>
//
// No DOM or app imports here: the worker and the tests load this file.

/** Sources larger than this aren't rendered (labeled), as in the preview. */
export const MAX_MATH = 10 * 1024;

type Katex = typeof import("katex").default;

/** KaTeX's options for the preview and exports. */
export function katexOptions(display: boolean) {
  return {
    displayMode: display,
    throwOnError: true,
    // No \href, \url, \htmlClass and friends: math can't link or style
    // the page.
    trust: false,
    strict: "ignore",
    maxExpand: 1000,
    maxSize: 100,
    output: "htmlAndMathml",
  } as const;
}

export interface MathLabels {
  /** Badge on an expression KaTeX refused. */
  error: string;
  /** Badge and report reason for an expression over MAX_MATH. */
  tooLarge: string;
}

export interface MathReport {
  kind: "math";
  source: string;
  reason: string;
}

const MATH =
  /<pre><code class="language-math" data-math-style="display">([^<]*)<\/code><\/pre>|<span data-math-style="(inline|display)">([^<]*)<\/span>/g;
const DIAGRAM = /<pre data-lang="mermaid"[^>]*><code[^>]*>([^<]*)<\/code><\/pre>/g;

/** Text as the core escaped it (comrak escapes `"&<>`), back to text. */
export function unescapeText(s: string): string {
  if (!s.includes("&")) return s;
  return s.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|quot|amp|lt|gt|apos);/g, (m, e: string) => {
    if (e[0] === "#") {
      const n = e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      return n > 0 && n <= 0x10ffff ? String.fromCodePoint(n) : m;
    }
    return { quot: '"', amp: "&", lt: "<", gt: ">", apos: "'" }[e] ?? m;
  });
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

type Rendered = { html: string } | { error: string };

/** Renders the math in `html` with KaTeX, as `renderMathElement` does in
 *  the preview: rendered expressions replace their element (display math
 *  in a code block replaces the `<pre>`); refused ones keep their source
 *  with a badge and are reported. `cache` is keyed by style and source. */
export function renderMathHtml(
  html: string,
  katex: Katex,
  labels: MathLabels,
  cache: Map<string, Rendered>,
  report: MathReport[],
): { html: string; math: number } {
  let math = 0;
  const out = html.replace(
    MATH,
    (_m: string, blockSrc?: string, style?: string, inlineSrc?: string) => {
      math++;
      const block = blockSrc !== undefined;
      const display = block || style === "display";
      const escaped = (block ? blockSrc : inlineSrc) ?? "";
      const src = unescapeText(escaped);
      const badged = (cls: string, badge: string) =>
        block
          ? `<pre><code class="language-math ${cls}" data-math-style="display">${badge}${escaped}</code></pre>`
          : `<span data-math-style="${style ?? "inline"}" class="${cls}">${badge}${escaped}</span>`;
      if (src.length > MAX_MATH) {
        report.push({ kind: "math", source: src.slice(0, 120), reason: labels.tooLarge });
        return badged(
          "md-rich-plain",
          `<span class="md-rich-badge">${escapeHtml(labels.tooLarge)}</span>`,
        );
      }
      const key = `${display ? "D" : "I"}${src}`;
      let r = cache.get(key);
      if (!r) {
        try {
          r = { html: katex.renderToString(src, katexOptions(display)) };
        } catch (e) {
          r = { error: e instanceof Error ? e.message : String(e) };
        }
        if (cache.size >= 4096) cache.clear();
        cache.set(key, r);
      }
      if ("error" in r) {
        report.push({ kind: "math", source: src.slice(0, 120), reason: r.error });
        return badged(
          "md-rich-error",
          `<span class="md-rich-badge" title="${escapeHtml(r.error)}">${escapeHtml(labels.error)}</span>`,
        );
      }
      return display
        ? `<div class="md-math md-math-display">${r.html}</div>`
        : `<span class="md-math">${r.html}</span>`;
    },
  );
  return { html: out, math };
}

/** Splits `html` at its Mermaid blocks: HTML pieces and, between them,
 *  each diagram's source (as an object). */
export function splitDiagrams(html: string): (string | { diagram: string })[] {
  const out: (string | { diagram: string })[] = [];
  let last = 0;
  for (const m of html.matchAll(DIAGRAM)) {
    out.push(html.slice(last, m.index));
    out.push({ diagram: unescapeText(m[1] ?? "") });
    last = m.index + m[0].length;
  }
  out.push(html.slice(last));
  return out;
}
