// PDF UI flows (M3 task 18), driven through the real app with WebDriver
// (ADR-0020). Run with `pwsh tools/webdriver/run-ui-tests.ps1 -Tests pdf-flows`.
//
// QUIRE_APP: path to quire.exe. QUIRE_UI_DIR: a scratch directory. The app
// must find PDFium (fetched by tools/pdfium/fetch-pdfium.ps1) and run with
// QUIRE_PDF_FAULTS=1 for the crash-recovery flow (the run script sets it).

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { after, before, test } from "node:test";
import { KEYS, WebDriver } from "./webdriver.mjs";

const APP = process.env.QUIRE_APP;
const DIR = process.env.QUIRE_UI_DIR;
if (!APP || !DIR) throw new Error("QUIRE_APP and QUIRE_UI_DIR must be set");

const wd = new WebDriver();
const pdfDir = path.join(DIR, "pdf");
const book = path.join(pdfDir, "book.pdf");
const broken = path.join(pdfDir, "broken.pdf");
const PAGES = 40;

/** Page i's square color, as in crates/pdf/tests/pool.rs. */
function color(i) {
  return [(i % 4) / 3, (Math.floor(i / 4) % 4) / 3, 0.5];
}

/**
 * A PDF with `pages` A4 pages; page i has a 100 pt square at (100, 100)
 * from the top left in color(i), and every 5th page is landscape.
 */
function makePdf(pages, { badStartxref = false } = {}) {
  const objs = [];
  const add = (body) => {
    objs.push(body);
    return objs.length;
  };
  const catalog = add(null);
  const tree = add(null);
  const kids = [];
  for (let i = 0; i < pages; i++) {
    const [w, h] = i % 5 === 4 ? [842, 595] : [595, 842];
    const [r, g, b] = color(i);
    const stream = `${r.toFixed(4)} ${g.toFixed(4)} ${b.toFixed(4)} rg 100 ${h - 200} 100 100 re f\n`;
    const content = add(`<< /Length ${stream.length} >>\nstream\n${stream}endstream`);
    kids.push(
      add(
        `<< /Type /Page /Parent ${tree} 0 R /MediaBox [0 0 ${w} ${h}] /Contents ${content} 0 R >>`,
      ),
    );
  }
  objs[catalog - 1] = `<< /Type /Catalog /Pages ${tree} 0 R >>`;
  objs[tree - 1] =
    `<< /Type /Pages /Count ${pages} /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] >>`;
  let out = "%PDF-1.7\n";
  const offsets = [];
  objs.forEach((body, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`;
  for (const o of offsets) out += `${String(o).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${badStartxref ? 7 : xref}\n%%EOF\n`;
  return Buffer.from(out, "latin1");
}

const status = () => wd.exec("return document.querySelector('.statusbar')?.innerText ?? ''");
const pdf = (expr) => wd.exec(`const v = window.__quire?.pdf?.view; return v ? (${expr}) : null`);

/** Waits until every visible tile at the current scale is drawn sharp. */
async function sharp(what, ms = 15_000) {
  await wd.until(async () => (await pdf("v.settled()")) === true, ms, what);
}

/** The drawn color at the center of page i's square, 0..1 per channel. */
async function squareColor(i) {
  // Bring the page's top into view, then wait for it to be drawn.
  await pdf(`v.gotoPage(${i})`);
  await sharp(`page ${i + 1} drawn`);
  return pdf(`(() => {
    const b = v.pageOnScreen(${i});
    if (!b) return null;
    const s = b.width / (${i} % 5 === 4 ? 842 : 595);
    return v.pixel(b.x + 150 * s, b.y + 150 * s).map((c) => c / 255);
  })()`);
}

function assertColor(got, i) {
  const want = color(i);
  assert.ok(got, `page ${i} on screen`);
  for (let c = 0; c < 3; c++) {
    assert.ok(Math.abs(got[c] - want[c]) < 0.02, `page ${i}: drew ${got}, want ${want}`);
  }
}

before(() => {
  fs.mkdirSync(pdfDir, { recursive: true });
  fs.writeFileSync(book, makePdf(PAGES));
  fs.writeFileSync(broken, makePdf(3, { badStartxref: true }));
});

after(async () => {
  await wd.stop();
});

test("a PDF given on the command line opens with page 1 drawn sharp", async () => {
  await wd.start(APP, [book]);
  await wd.exec(
    "window.__errors = []; window.addEventListener('error', (e) => window.__errors.push(String(e.message))); window.addEventListener('unhandledrejection', (e) => window.__errors.push(String(e.reason)));",
  );
  await wd.until(async () => /Page\s+of 40/.test(await status()), 20_000, "page count");
  await sharp("first page sharp");
  assertColor(await squareColor(0), 0);
  // The background walk learns every page's size.
  await wd.until(async () => !/Measuring/.test(await status()), 10_000, "page walk done");
  assert.equal(await pdf("v.state().estimated"), false);
  assert.deepEqual(await wd.exec("return window.__errors"), []);
});

test("go to page jumps there and draws it", async () => {
  const input = await wd.find(".pdf-page input");
  await wd.click(input);
  await wd.type(input, `30${KEYS.enter}`);
  await wd.until(async () => (await pdf("v.state().page")) === 29, 5000, "page 30 current");
  await sharp("page 30 sharp");
  assertColor(await squareColor(29), 29);
  // Landscape pages are laid out wider than tall.
  const b = await pdf("v.pageOnScreen(29)");
  assert.ok(b.width > b.height, "page 30 is landscape");
});

test("zooming in draws sharp tiles at the new scale", async () => {
  await pdf("v.setZoom(400)");
  await wd.until(async () => /400 %/.test(await status()), 5000, "zoom shown");
  await sharp("sharp at 400 %");
  await pdf("v.setZoom(1600)");
  await sharp("sharp at 1600 %");
  await pdf("v.fit('fitWidth')");
  await sharp("sharp at fit width");
});

test("rotating swaps the page's width and height", async () => {
  const page = await pdf("v.state().page");
  const before = await pdf(`v.pageOnScreen(${page})`);
  await pdf("v.rotate(90)");
  await sharp("sharp after rotating");
  const afterBox = await pdf(`v.pageOnScreen(${page})`);
  assert.ok(
    before.width > before.height !== afterBox.width > afterBox.height,
    `rotated: ${JSON.stringify([before, afterBox])}`,
  );
  await pdf("v.rotate(-90)");
  await sharp("sharp after rotating back");
});

test("two-page and single-page layouts", async () => {
  await pdf("v.setMode('twoUp')");
  await sharp("two-up sharp");
  const a = await pdf(
    "v.pageOnScreen(0) && v.pageOnScreen(1) && [v.pageOnScreen(0), v.pageOnScreen(1)]",
  );
  if (a) assert.equal(Math.round(a[0].y + a[0].height / 2), Math.round(a[1].y + a[1].height / 2));
  await pdf("v.setMode('single')");
  await pdf("v.gotoPage(6)");
  await sharp("single page sharp");
  assert.equal(await pdf("v.pageOnScreen(5)"), null, "only the current page is laid out");
  assertColor(await squareColor(6), 6);
  await pdf("v.setMode('continuous')");
});

test("a crashed worker is restarted and the tiles come back", async () => {
  await wd.execAsync(
    "const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke('pdf_crash_worker').then(() => done(true), (e) => done(String(e)))",
  );
  // New tiles after the crash: a scale no tile has been rendered at.
  await pdf("v.setZoom(237)");
  await sharp("sharp after a worker crash");
  const stats = await wd.execAsync(
    "const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke('pdf_stats').then(done, (e) => done(String(e)))",
  );
  assert.ok(stats.crashes >= 1, JSON.stringify(stats));
  assertColor(await squareColor(await pdf("v.state().page")), await pdf("v.state().page"));
});

test("a PDF with a broken cross-reference table opens repaired, with a banner", async () => {
  await wd.exec("return window.__quire.open(arguments[0])", [broken]);
  await wd.until(async () => /of 3/.test(await status()), 10_000, "repaired file open");
  await wd.until(
    () => wd.exec("return document.querySelector('.banner')?.innerText ?? ''"),
    5000,
    "repaired banner",
  );
  await sharp("repaired file sharp");
  assertColor(await squareColor(0), 0);
});
