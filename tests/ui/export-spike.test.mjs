// Export spike (M2 task 11, ADR-0031): HTML and PDF export of the 20 MiB
// Markdown fixture through the real app, timed. A measurement, not a flow:
// not in the default list. Run with
//   pwsh tools/webdriver/run-ui-tests.ps1 -NoBuild -Tests export-spike
// QUIRE_SPIKE_FIXTURE overrides the fixture (default
// .cache/fixtures/document-20mib.md); results go to QUIRE_UI_DIR/export-spike.json.

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
       (err) => { window.__exported = { error: String(err) }; });`,
    [kind, file],
  );
  const r = await wd.until(() => wd.exec("return window.__exported"), 15 * 60_000, kind);
  if (r.error) throw new Error(r.error);
  return r;
}

/** Long tasks while `fn` runs (the page's own main-thread work). */
async function withLongTasks(fn) {
  await wd.exec(
    `window.__long = []; new PerformanceObserver((l) => { for (const e of l.getEntries()) window.__long.push(e.duration); }).observe({ type: "longtask" });`,
  );
  const r = await fn();
  const long = await wd.exec("return window.__long");
  return { ...r, longTasks: long.length, longestTaskMs: Math.max(0, ...long) };
}

test("export spike: the 20 MiB fixture to HTML and PDF", { timeout: 40 * 60_000 }, async () => {
  assert.ok(fs.existsSync(fixture), `${fixture} (generate it with tools/fixtures)`);
  await wd.start(APP, [fixture]);
  await wd.until(() => wd.exec("return !!window.__quire?.session?.md"), 60_000, "Markdown session");
  const html = path.join(DIR, "spike-export.html");
  const pdf = path.join(DIR, "spike-export.pdf");
  const h = await withLongTasks(() => exportTo("html", html));
  const p = await withLongTasks(() => exportTo("pdf", pdf));
  let pages = null;
  try {
    pages = Number(execFileSync("qpdf", ["--show-npages", pdf]).toString().trim());
  } catch {
    // qpdf not on PATH: pages unknown.
  }
  const reportKinds = (r) =>
    r.report.reduce((acc, x) => ({ ...acc, [x.kind]: (acc[x.kind] ?? 0) + 1 }), {});
  const out = {
    fixture: path.basename(fixture),
    fixtureBytes: fs.statSync(fixture).size,
    html: {
      ms: Math.round(h.ms),
      bytes: fs.statSync(html).size,
      longTasks: h.longTasks,
      longestTaskMs: Math.round(h.longestTaskMs),
      report: reportKinds(h),
    },
    pdf: {
      ms: Math.round(p.ms),
      bytes: fs.statSync(pdf).size,
      pages,
      longTasks: p.longTasks,
      longestTaskMs: Math.round(p.longestTaskMs),
      report: reportKinds(p),
    },
  };
  fs.writeFileSync(path.join(DIR, "export-spike.json"), JSON.stringify(out, null, 2));
  console.log(JSON.stringify(out));
  assert.equal(fs.readFileSync(pdf).subarray(0, 5).toString("latin1"), "%PDF-");
});
