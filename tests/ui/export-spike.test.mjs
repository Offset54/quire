// Export spike (M2 task 11, ADR-0031): HTML and PDF export of the 20 MiB
// Markdown fixture through the real app, timed. A measurement, not a flow:
// not in the default list. Run with
//   pwsh tools/webdriver/run-ui-tests.ps1 -NoBuild -Tests export-spike
// QUIRE_SPIKE_FIXTURE overrides the fixture (default
// .cache/fixtures/document-20mib.md), QUIRE_SPIKE_PDF=0 skips the PDF (refused
// over a 10 MiB page); results go to QUIRE_UI_DIR/export-spike.json.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { WebDriver } from "./webdriver.mjs";

const APP = process.env.QUIRE_APP;
const DIR = process.env.QUIRE_UI_DIR;
if (!APP || !DIR) throw new Error("QUIRE_APP and QUIRE_UI_DIR must be set");
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const fixture =
  process.env.QUIRE_SPIKE_FIXTURE ?? path.join(root, ".cache/fixtures/document-20mib.md");

const wd = new WebDriver();

after(async () => {
  await wd.stop();
});

/** Runs `md.exportTo(kind, file)`; resolves with the report and the time. */
async function exportTo(kind, file) {
  await wd.exec(
    `window.__exported = undefined;
     const t0 = performance.now();
     window.__quire.session.md.exportTo(arguments[0], arguments[1]).then(
       (report) => { window.__exported = { report, ms: performance.now() - t0 }; },
       (err) => { window.__exported = { failed: String(err) }; });`,
    [kind, file],
  );
  const r = await wd.until(() => wd.exec("return window.__exported"), 15 * 60_000, kind);
  if (r.failed) throw new Error(r.failed);
  return r;
}

/** Long tasks while `fn` runs (the page's own main-thread work). */
async function withLongTasks(fn) {
  // Long tasks (the API's floor is 50 ms) and, below that, gaps in a
  // MessageChannel ping loop: a gap is the time other tasks held the main
  // thread between two pings (§2: no task over 8 ms).
  await wd.exec(
    `window.__long = []; window.__lo?.disconnect();
     window.__lo = new PerformanceObserver((l) => { for (const e of l.getEntries()) window.__long.push(e.duration); });
     window.__lo.observe({ type: "longtask" });
     window.__loaf = []; window.__lf?.disconnect();
     window.__lf = new PerformanceObserver((l) => {
       for (const e of l.getEntries()) {
         window.__loaf.push({
           ms: Math.round(e.duration),
           scripts: e.scripts.map((x) => ({ ms: Math.round(x.duration), invoker: x.invoker, src: x.sourceURL.split("/").pop(), fn: x.sourceFunctionName })),
         });
       }
     });
     try { window.__lf.observe({ type: "long-animation-frame" }); } catch {}
     const t0 = performance.now();
     const gaps = (window.__gaps = { n: 0, over8: 0, over16: 0, max: 0, run: true, big: [] });
     const ch = new MessageChannel();
     let last = performance.now();
     ch.port1.onmessage = () => {
       const now = performance.now();
       const g = now - last;
       last = now;
       gaps.n++;
       if (g > 8) gaps.over8++;
       if (g > 16) gaps.over16++;
       if (g > 8) gaps.big.push([Math.round(now - t0), Math.round(g)]);
       if (g > gaps.max) gaps.max = g;
       if (gaps.run) ch.port2.postMessage(0);
     };
     ch.port2.postMessage(0);`,
  );
  const r = await fn();
  const long = await wd.exec("return window.__long");
  const gaps = await wd.exec("window.__gaps.run = false; return window.__gaps");
  const loaf = await wd.exec("return window.__loaf");
  long.sort((a, b) => b - a);
  return {
    ...r,
    gapsOver8Ms: gaps.over8,
    gapsOver16Ms: gaps.over16,
    maxGapMs: Math.round(gaps.max * 10) / 10,
    /** [ms since the start, gap ms] of each gap over 8 ms. */
    gapsAt: gaps.big.slice(0, 20),
    loaf: loaf.slice(0, 5),
    longTasks: long.length,
    longestTaskMs: long[0] ?? 0,
    topTasksMs: long.slice(0, 5).map(Math.round),
    longTaskTotalMs: Math.round(long.reduce((a, b) => a + b, 0)),
  };
}

test("export spike: the 20 MiB fixture to HTML and PDF", { timeout: 40 * 60_000 }, async () => {
  assert.ok(fs.existsSync(fixture), `${fixture} (generate it with tools/fixtures)`);
  await wd.start(APP, [fixture]);
  await wd.until(() => wd.exec("return !!window.__quire?.session?.md"), 60_000, "Markdown session");
  const reportKinds = (r) =>
    r.report.reduce((acc, x) => ({ ...acc, [x.kind]: (acc[x.kind] ?? 0) + 1 }), {});
  const run = async (kind, file) => {
    const r = await withLongTasks(() => exportTo(kind, file));
    const { report, ms, ...rest } = r;
    return {
      ms: Math.round(ms),
      bytes: fs.statSync(file).size,
      ...rest,
      report: reportKinds({ report }),
    };
  };
  const out = { fixture: path.basename(fixture), fixtureBytes: fs.statSync(fixture).size };
  // The page's own work after opening, with no export (the baseline the
  // export's numbers are read against).
  out.idleAfterOpen = await withLongTasks(async () => {
    await new Promise((r) => setTimeout(r, 3000));
    return { ms: 3000 };
  });
  // The first export in the session, then the same again (warm: worker,
  // KaTeX and the frame's code already compiled).
  out.html = await run("html", path.join(DIR, "spike-export.html"));
  out.htmlWarm = await run("html", path.join(DIR, "spike-export-2.html"));
  if (process.env.QUIRE_SPIKE_PDF !== "0") {
    const pdf = path.join(DIR, "spike-export.pdf");
    out.pdf = await run("pdf", pdf);
    try {
      out.pdf.pages = Number(execFileSync("qpdf", ["--show-npages", pdf]).toString().trim());
    } catch {
      // qpdf not on PATH: pages unknown.
    }
    assert.equal(fs.readFileSync(pdf).subarray(0, 5).toString("latin1"), "%PDF-");
  }
  fs.writeFileSync(path.join(DIR, "export-spike.json"), JSON.stringify(out, null, 2));
  console.log(JSON.stringify(out));
});
