// Markdown UI flows (M2 task 14), driven through the real app with
// WebDriver (ADR-0020). Run with `pwsh tools/webdriver/run-ui-tests.ps1`.
//
// QUIRE_APP: path to quire.exe. QUIRE_UI_DIR: a scratch directory.

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { after, before, test } from "node:test";
import { KEYS, WebDriver } from "./webdriver.mjs";

const APP = process.env.QUIRE_APP;
const DIR = process.env.QUIRE_UI_DIR;
if (!APP || !DIR) throw new Error("QUIRE_APP and QUIRE_UI_DIR must be set");

const wd = new WebDriver();
const mdDir = path.join(DIR, "md");
const sample = path.join(mdDir, "notes.md");

/** A small valid PNG (solid color), so the image scheme has something to serve. */
function png(w, h) {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (buf) => {
    let c = 0xffffffff;
    for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const c = Buffer.alloc(4);
    c.writeUInt32BE(crc(td));
    return Buffer.concat([len, td, c]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) raw.set([40, 120, 200], y * (w * 3 + 1) + 1 + x * 3);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function writeSample() {
  fs.mkdirSync(path.join(mdDir, "img"), { recursive: true });
  fs.writeFileSync(path.join(mdDir, "img", "blue.png"), png(64, 32));
  const parts = [
    "# Notes title\n",
    "First paragraph with a [reference link][far] and **bold** text.\n",
    "![local image](img/blue.png)\n",
    "![tracker](https://tracker.invalid/pixel.png)\n",
    "| a | b |\n|---|---|\n| 1 | 2 |\n",
    '<script>window.__pwned = true</script>\n\n<div onclick="window.__pwned = true">raw div</div>\n',
    "```rust\nfn main() { let answer = 42; }\n```\n",
    "```cobol\nDISPLAY 'HI'.\n```\n",
    "Inline $x^2 + y_1$ and broken $\\nosuchcommand{x}$ math.\n",
    "$$\n\\int_0^1 f(t)\\,dt\n$$\n",
    "```mermaid\ngraph TD\n  A[Start] --> B[Done]\n```\n",
    "```mermaid\nthis is not a diagram\n```\n",
  ];
  for (let i = 0; i < 400; i++) {
    parts.push(`## Section ${i}\n`);
    parts.push(`Paragraph ${i} with some words to fill the line, and more words after them.\n`);
    if (i % 10 === 5) parts.push("- item one\n- item two\n  continued\n");
  }
  // A table large enough to be shown as pieces (crates/markdown/src/pieces.rs).
  const rows = ["| Name | Value | Note |\n|---|--:|---|"];
  for (let i = 0; i < 300; i++) rows.push(`| row ${i} | ${i * 3} | 한국어 ${i % 7} |`);
  parts.push(rows.join("\n") + "\n");
  parts.push("[far]: https://example.com/far\n");
  fs.writeFileSync(sample, parts.join("\n"));
}

/** Text of the preview blocks on screen, top to bottom. */
function previewBlocks() {
  return wd.exec(`
    const host = document.querySelector('.md-viewport');
    if (!host || host.offsetParent === null) return [];
    const vp = host.getBoundingClientRect();
    return [...document.querySelectorAll('.md-block')]
      .map((b) => ({ r: b.getBoundingClientRect(), t: b.innerText, start: Number(b.dataset.start) }))
      .filter((b) => b.r.bottom > vp.top && b.r.top < vp.bottom && b.r.height > 0)
      .sort((a, b) => a.r.top - b.r.top)
      .map((b) => ({ t: b.t, start: b.start, top: b.r.top - vp.top }));
  `);
}

function sourceRows() {
  return wd.exec(`
    const vp = document.querySelector('.txt-viewport');
    if (!vp || vp.offsetParent === null) return [];
    const r0 = vp.getBoundingClientRect();
    return [...document.querySelectorAll('.txt-row:not(.txt-probe)')]
      .filter((r) => r.style.display !== 'none')
      .map((r) => ({ y: r.getBoundingClientRect().top, t: r.querySelector('.txt-text').textContent }))
      .filter((r) => r.y >= r0.top - 1 && r.y < r0.bottom)
      .sort((a, b) => a.y - b.y);
  `);
}

/** The section number shown first in `texts` (a heading or its paragraph). */
function firstSection(texts) {
  const m = /(?:Section|Paragraph) (\d+)/.exec(texts.join("\n"));
  return m ? Number(m[1]) : undefined;
}

const mode = () => wd.exec("return document.querySelector('.editor-panes')?.dataset.mode ?? ''");
const status = () => wd.exec("return document.querySelector('.statusbar')?.innerText ?? ''");
const focusEditor = () => wd.exec("document.querySelector('.txt-input').focus()");

async function keys(text) {
  const actions = [];
  for (const ch of text) actions.push({ type: "keyDown", value: ch }, { type: "keyUp", value: ch });
  await wd.cmd("POST", "/actions", { actions: [{ type: "key", id: "kb", actions }] });
  await wd.cmd("DELETE", "/actions");
}

before(() => {
  writeSample();
});

/** State for a failure message. */
async function dump() {
  return JSON.stringify({
    preview: (await previewBlocks()).slice(0, 3),
    source: (await sourceRows()).slice(0, 3),
    notices: await wd.exec("return document.querySelector('.notices')?.innerText ?? ''"),
    errors: await wd.exec("return window.__errors ?? []"),
    offsets: await wd.exec(
      "const s = window.__quire?.session; return s ? { preview: s.md?.preview.topOffset(), source: s.view?.scrollOffset() } : null",
    ),
  });
}

async function untilOrDump(fn, ms, what) {
  try {
    return await wd.until(fn, ms, what);
  } catch (err) {
    throw new Error(`${String(err)} ${await dump()}`, { cause: err });
  }
}

after(async () => {
  await wd.stop();
});

test("a .md file opens in the editing view with the Markdown status items", async () => {
  await wd.start(APP, [sample]);
  await wd.exec(
    "window.__errors = []; window.addEventListener('error', (e) => window.__errors.push(String(e.message))); window.addEventListener('unhandledrejection', (e) => window.__errors.push(String(e.reason)));",
  );
  await wd.until(async () => (await sourceRows()).length > 5, 20_000, "source rows");
  assert.equal(await mode(), "source");
  const r = await sourceRows();
  assert.equal(r[0].t, "# Notes title");
  await wd.until(async () => /Reading/.test(await status()), 10_000, "view mode buttons");
});

test("split view shows both panes, and the preview renders the blocks", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "\\"]);
  await wd.until(async () => (await mode()) === "split", 5000, "split mode");
  const blocks = await wd.until(
    async () => {
      const b = await previewBlocks();
      return b.length > 3 && b;
    },
    10_000,
    "preview blocks",
  );
  assert.equal(blocks[0].t.trim(), "Notes title");
  assert.ok((await sourceRows()).length > 5, "source still shown");
  // The definition at the end resolves the reference link at the top.
  await wd.until(
    () =>
      wd.exec("return !!document.querySelector('.md-block a[href=\"https://example.com/far\"]')"),
    10_000,
    "reference resolved from the end of the document",
  );
  // Raw HTML was sanitized: no script ran, no handler kept.
  assert.equal(await wd.exec("return window.__pwned === true"), false);
  assert.equal(await wd.exec("return !!document.querySelector('.md-block [onclick]')"), false);
  assert.equal(await wd.exec("return !!document.querySelector('.md-block script')"), false);
});

test("a local image loads through the image scheme; a remote one is a placeholder", async () => {
  await wd.until(
    () =>
      wd.exec(
        "const i = document.querySelector('.md-block img'); return !!i && i.complete && i.naturalWidth === 64",
      ),
    10_000,
    "local image decoded",
  );
  const src = await wd.exec("return document.querySelector('.md-block img').getAttribute('src')");
  assert.match(src, /quire-img/);
  assert.match(
    await wd.exec("return document.querySelector('.md-image-blocked')?.innerText ?? ''"),
    /tracker\.invalid/,
  );
  await wd.until(
    () =>
      wd.exec("return /remote images/.test(document.querySelector('.banner')?.innerText ?? '')"),
    5000,
    "remote-content banner",
  );
});

test("the remote-content toggle renders the document again with remote images allowed", async () => {
  await wd.exec(
    "[...document.querySelectorAll('.banner button')].find((b) => b.innerText === 'Load remote content').click()",
  );
  await wd.until(async () => /Remote content on/.test(await status()), 5000, "toggle on");
  // tracker.invalid can't resolve: the placeholder now says the image failed.
  await wd.until(
    () =>
      wd.exec(
        "return [...document.querySelectorAll('.md-image-blocked')].some((e) => e.classList.contains('error'))",
      ),
    20_000,
    "remote image attempted",
  );
  await wd.exec(
    "[...document.querySelectorAll('.statusbar button')].find((b) => b.innerText === 'Remote content on').click()",
  );
  await wd.until(async () => !/Remote content on/.test(await status()), 5000, "toggle off");
});

test("typing in the source updates the preview", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, KEYS.home]);
  await keys("Hello ");
  await wd.until(
    async () => ((await previewBlocks())[0]?.t ?? "").includes("Hello # Notes title"),
    5000,
    "preview updated",
  );
});

test("scrolling the preview scrolls the source (and back)", async () => {
  await wd.exec(`
    const vp = document.querySelector('.md-viewport');
    for (let i = 0; i < 10; i++) vp.dispatchEvent(new WheelEvent('wheel', { deltaY: 400, bubbles: true, cancelable: true }));
  `);
  const top = await wd.until(
    async () => {
      const n = firstSection((await previewBlocks()).map((b) => b.t));
      return n !== undefined && n > 5 && n;
    },
    5000,
    "preview scrolled",
  );
  await untilOrDump(
    async () => {
      const n = firstSection((await sourceRows()).map((r) => r.t));
      return n !== undefined && Math.abs(n - top) <= 3;
    },
    5000,
    "source followed",
  );
  await wd.exec(`
    const vp = document.querySelector('.txt-viewport');
    for (let i = 0; i < 10; i++) vp.dispatchEvent(new WheelEvent('wheel', { deltaY: 400, bubbles: true, cancelable: true }));
  `);
  await wd.until(
    async () => {
      const n = firstSection((await previewBlocks()).map((b) => b.t));
      return n !== undefined && n > top + 5;
    },
    5000,
    "preview followed",
  );
});

test("Ctrl+E switches to the reading view and back, keeping the position", async () => {
  const before = firstSection((await previewBlocks()).map((b) => b.t));
  assert.ok(before !== undefined && before > 5, "scrolled into the document");
  const near = (n) => n !== undefined && Math.abs(n - before) <= 1;
  await focusEditor();
  await wd.chord([KEYS.control, "e"]);
  await wd.until(async () => (await mode()) === "preview", 5000, "reading view");
  assert.equal((await sourceRows()).length, 0, "source hidden");
  // The same section (the pane got wider, so a neighbour may lead).
  await untilOrDump(
    async () => near(firstSection((await previewBlocks()).map((b) => b.t))),
    5000,
    `same position (from section ${before})`,
  );
  await wd.chord([KEYS.control, "e"]);
  await wd.until(async () => (await mode()) === "source", 5000, "editing view");
  await untilOrDump(
    async () => near(firstSection((await sourceRows()).map((r) => r.t))),
    5000,
    `source at the same place (from section ${before})`,
  );
});

test("save writes the edit and leaves the rest byte-exact", async () => {
  const original = fs.readFileSync(sample, "utf8");
  await focusEditor();
  await wd.chord([KEYS.control, "s"]);
  await wd.until(async () => /Saved/.test(await status()), 10_000, "saved");
  const disk = fs.readFileSync(sample, "utf8");
  assert.equal(disk, "Hello " + original);
});

// After the save: the preview reads the saved file through a new
// snapshot (an old one would read it shifted by the unsaved edits).
test("code is highlighted after the paint; other languages are labeled plain", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "\\"]);
  await wd.until(async () => (await mode()) === "split", 5000, "split mode");
  const at = fs.readFileSync(sample).indexOf(Buffer.from("```rust"));
  await wd.exec(`window.__quire.session.md.preview.reveal(${at})`);
  const code = await untilOrDump(
    () =>
      wd.exec(`
        const rust = document.querySelector('.md-block pre[data-lang="rust"]');
        const cobol = document.querySelector('.md-block pre[data-lang="cobol"]');
        if (!rust || !cobol || !rust.querySelector('.hl-keyword')) return false;
        return {
          keyword: rust.querySelector('.hl-keyword').textContent,
          text: rust.querySelector('code').textContent,
          rustHl: rust.dataset.hl ?? null,
          cobolHl: cobol.dataset.hl ?? null,
          label: getComputedStyle(cobol, '::before').content,
        };
      `),
    10_000,
    "highlighted code",
  );
  assert.ok(["fn", "let"].includes(code.keyword), code.keyword);
  assert.equal(code.text, "fn main() { let answer = 42; }\n");
  assert.equal(code.rustHl, null);
  assert.equal(code.cobolHl, "none");
  assert.equal(code.label, '"cobol"');
  // Leave the view mode as the next flow expects it.
  await focusEditor();
  await wd.chord([KEYS.control, "\\"]);
  await wd.until(async () => (await mode()) !== "split", 5000, "split mode off");
});

test("math renders with KaTeX and diagrams in the sandboxed frame; errors keep the source", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "\\"]);
  await wd.until(async () => (await mode()) === "split", 5000, "split mode");
  const at = fs.readFileSync(sample).indexOf(Buffer.from("Inline $x^2"));
  await wd.exec(`window.__quire.session.md.preview.reveal(${at})`);
  let r;
  try {
    r = await untilOrDump(
      () =>
        wd.exec(`
        const vp = document.querySelector('.md-viewport');
        const inline = vp.querySelectorAll('.md-math:not(.md-math-display) .katex').length;
        const display = vp.querySelectorAll('.md-math-display .katex-display').length;
        const mathErr = vp.querySelector('[data-math-style].md-rich-error .md-rich-badge');
        const fig = vp.querySelector('figure.md-diagram img');
        const diagErr = vp.querySelector('pre[data-lang="mermaid"].md-rich-error .md-rich-badge');
        window.__richProbe = {
          inline, display, mathErr: !!mathErr, fig: !!fig, diagErr: !!diagErr,
          math: [...vp.querySelectorAll('[data-math-style]')].map((e) => e.dataset.rich ?? '-'),
          mermaid: [...vp.querySelectorAll('pre[data-lang="mermaid"]')].map((e) => (e.dataset.rich ?? '-') + ' ' + (e.querySelector('.md-rich-badge')?.title ?? '')),
          frames: document.querySelectorAll('iframe.md-diagram-frame').length,
          texts: [...vp.querySelectorAll('.md-block')].map((b) => b.innerText.slice(0, 30)),
        };
        if (!inline || !display || !mathErr || !fig || !fig.complete || !diagErr) return false;
        return {
          inline, display,
          mathErrSource: mathErr.parentElement.textContent,
          figSrc: fig.getAttribute('src').slice(0, 26),
          figWidth: fig.naturalWidth,
          diagErrTitle: diagErr.title,
          frames: [...document.querySelectorAll('iframe.md-diagram-frame')].map((f) => f.getAttribute('sandbox')),
          scripts: vp.querySelectorAll('script, foreignObject').length,
        };
      `),
      20_000,
      "math and diagrams",
    );
  } catch (err) {
    throw new Error(
      `${String(err)} ${JSON.stringify(await wd.exec("return window.__richProbe"))}`,
      {
        cause: err,
      },
    );
  } finally {
    await focusEditor();
    await wd.chord([KEYS.control, "\\"]);
    await wd.until(async () => (await mode()) !== "split", 5000, "split mode off");
  }
  assert.equal(r.inline, 1);
  assert.equal(r.display, 1);
  assert.match(r.mathErrSource, /nosuchcommand/);
  assert.equal(r.figSrc, "data:image/svg+xml;charset");
  assert.ok(r.figWidth > 0, "the diagram image decoded");
  assert.ok(r.diagErrTitle.length > 0, "the error message is the badge's title");
  assert.deepEqual(r.frames, ["allow-scripts"]);
  assert.equal(r.scripts, 0);
  // The app's document keeps its prototype frozen (src/freeze.ts).
  assert.equal(await wd.exec("return Object.isFrozen(Object.prototype)"), true);
});

test("a large table shows as aligned pieces with one header", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "\\"]);
  await wd.until(async () => (await mode()) === "split", 5000, "split mode");
  const at = fs.readFileSync(sample).indexOf(Buffer.from("| row 150 |"));
  await wd.exec(`window.__quire.session.md.preview.reveal(${at})`);
  const shape = await untilOrDump(
    () =>
      wd.exec(`
        const pieces = [...document.querySelectorAll('.md-piece-table')];
        if (pieces.length < 3) return false;
        const lefts = pieces.map((t) => [...(t.querySelector('tbody tr')?.children ?? [])].map((c) => Math.round(c.getBoundingClientRect().left)));
        const heads = [...document.querySelectorAll('.md-piece thead')].filter((h) => h.offsetParent !== null).length;
        const firstShown = !!document.querySelector('.md-piece-first');
        return { n: pieces.length, lefts, heads, firstShown, text: document.querySelector('.md-viewport').innerText.includes('row 150') };
      `),
    10_000,
    "table pieces",
  );
  assert.ok(shape.text, "row 150 is shown");
  for (const l of shape.lefts) assert.deepEqual(l, shape.lefts[0], "columns line up");
  assert.ok(shape.heads <= 1, "at most one header");
  if (shape.firstShown) assert.equal(shape.heads, 1);
});
