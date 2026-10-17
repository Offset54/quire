import katex from "katex";
import { describe, expect, it } from "vitest";
import {
  MAX_MATH,
  renderMathHtml,
  splitDiagrams,
  unescapeText,
  type MathReport,
} from "./math-core";

const labels = { error: "Math error", tooLarge: "Too large" };

// The core's markup for `Inline $a<b$ and $$e$$.`, a `$$` block, a
// ```math block and a ```mermaid block (render_document, sourcepos off).
const CORE = `<p>Inline <span data-math-style="inline">a&lt;b</span> and <span data-math-style="display">e</span>.</p>
<p><span data-math-style="display">
f
</span></p>
<pre><code class="language-math" data-math-style="display">g &gt; h
</code></pre>
<pre data-lang="mermaid" data-hl="none"><code class="language-mermaid">graph TD; A--&gt;B
</code></pre>
`;

function render(html: string) {
  const report: MathReport[] = [];
  const r = renderMathHtml(html, katex, labels, new Map(), report);
  return { ...r, report };
}

describe("export math as strings", () => {
  it("renders every form the core writes", () => {
    const r = render(CORE);
    expect(r.math).toBe(4);
    expect(r.report).toEqual([]);
    expect(r.html).not.toContain("data-math-style");
    expect(r.html).toContain('<span class="md-math"><span class="katex">');
    // `$$` and ```math: display math.
    expect(r.html.match(/<div class="md-math md-math-display">/g)).toHaveLength(3);
    // The ```math block replaces its <pre>; the Mermaid block is untouched.
    expect(r.html).not.toContain('<pre><code class="language-math"');
    expect(r.html).toContain('<pre data-lang="mermaid"');
    // The source was unescaped before KaTeX saw it.
    expect(r.html).toContain("<mo>&lt;</mo>");
  });

  it("keeps a refused expression's source with a badge and reports it", () => {
    const r = render('<p><span data-math-style="inline">\\frac{</span></p>');
    expect(r.report).toHaveLength(1);
    expect(r.report[0]?.source).toBe("\\frac{");
    expect(r.html).toMatch(
      /^<p><span data-math-style="inline" class="md-rich-error"><span class="md-rich-badge" title="[^"<]+">Math error<\/span>\\frac\{<\/span><\/p>$/,
    );
  });

  it("labels expressions over the size cap", () => {
    const src = "x".repeat(MAX_MATH + 1);
    const r = render(
      `<pre><code class="language-math" data-math-style="display">${src}</code></pre>`,
    );
    expect(r.report[0]?.reason).toBe("Too large");
    expect(r.html).toContain('class="language-math md-rich-plain"');
  });

  it("splits at diagrams, with their sources unescaped", () => {
    const parts = splitDiagrams(CORE);
    expect(parts).toHaveLength(3);
    expect(parts[1]).toEqual({ diagram: "graph TD; A-->B\n" });
    expect(parts[0]).toMatch(/<\/pre>\n$/);
    expect(parts[2]).toBe("\n");
  });

  it("unescapes the references comrak and the sanitizer write", () => {
    expect(unescapeText("a&amp;b&lt;c&gt;d&quot;e&#39;f&#x41;g&nbsp;")).toBe(
      "a&b<c>d\"e'fAg&nbsp;",
    );
  });
});
