// Font warm-up. The first layout that uses a font face loads it, and text
// in a script the stack's first font lacks also runs the platform's font
// fallback; on Windows that is 10–30 ms per face on the UI thread. Opening
// a document whose first screen mixes Latin, Hangul and emoji paid all of it
// in one task (60–78 ms in md-20m, over §4's 50 ms). A worker first measures
// the samples on an OffscreenCanvas, which loads the faces off the UI thread
// (the typeface cache is process-wide); then a sample of each script in each
// stack is laid out in the page, one per task, now 0–5 ms each. The worker
// starts after the first frame (at script start it slows warm start), or at
// script start when a command-line file is opening, so the faces are loaded
// before its first rows draw (ADR-0030 addendum). Scripts not sampled here
// still pay on first use (RISKS #13).

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

let workerDone: Promise<void> | undefined;

/** Starts warming the faces in a worker (src/fonts-worker.ts): called as the
 *  page's script starts, so the faces are loaded before a document opened
 *  from the command line draws its first rows. */
export function startFontWorker(): void {
  if (workerDone) return;
  const t0 = performance.now();
  workerDone = new Promise<void>((resolve) => {
    let worker: Worker | undefined;
    const done = () => {
      window.clearTimeout(timer);
      worker?.terminate();
      resolve();
    };
    const timer = window.setTimeout(done, 2000);
    try {
      worker = new Worker(new URL("./fonts-worker.ts", import.meta.url), { type: "module" });
      worker.onmessage = (e: MessageEvent<number[]>) => {
        performance.measure("font-warm-worker", {
          start: t0,
          end: performance.now(),
          detail: e.data.map(Math.round),
        });
        done();
      };
      worker.onerror = done;
    } catch {
      done();
    }
  });
}

/** Once the worker is done, lays out the samples in the page, one per task
 *  (what the worker's canvas didn't load, such as the page's own font
 *  matching, then costs little). */
export function warmFonts(): void {
  if (started) return;
  started = true;
  startFontWorker();
  void workerDone?.then(layOut);
}

function layOut(): void {
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
