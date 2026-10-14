// Font warm-up. The first layout that uses a font face loads it, and text
// in a script the stack's first font lacks also runs the platform's font
// fallback; on Windows that is 10–30 ms per face on the UI thread. Opening
// a document whose first screen mixes Latin, Hangul and emoji paid all of it
// in one task (60–78 ms in md-20m, over §4's 50 ms). Laying out a sample of
// each script in each stack, one per task after the first frame, moves that
// cost off the open path in pieces. Scripts not sampled here still pay on
// first use (RISKS #13).

/** (class of the stack's element, sample text) */
const SAMPLES: [string, string][] = [
  ["txt", "Straße naïve 0123 {}"],
  ["txt", "한국어 문서"],
  ["txt", "🙂"],
  ["markdown-body", "Straße naïve 0123"],
  ["markdown-body", "한국어 문서"],
  ["markdown-body", "🙂"],
  ["markdown-body md-warm-code", "Straße 한국어"],
];

let started = false;

/** Lays out the samples, one per task. */
export function warmFonts(): void {
  if (started) return;
  started = true;
  let i = 0;
  const step = () => {
    const s = SAMPLES[i++];
    if (!s) return;
    const t0 = performance.now();
    const el = document.createElement("div");
    el.className = s[0];
    el.setAttribute("aria-hidden", "true");
    el.style.cssText = "position:fixed;left:-10000px;top:0;visibility:hidden";
    const inner = document.createElement(s[0].includes("md-warm-code") ? "code" : "span");
    inner.textContent = s[1];
    el.append(inner);
    document.body.append(el);
    el.getBoundingClientRect(); // forces the layout
    el.remove();
    performance.measure("font-warm", { start: t0, end: performance.now() });
    setTimeout(step, 0);
  };
  setTimeout(step, 0);
}
