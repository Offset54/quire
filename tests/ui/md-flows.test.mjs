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
  // The previous flow's scrolling may still be settling: start from a
  // position that holds for 300 ms.
  let before;
  let last;
  await wd.until(
    async () => {
      const n = firstSection((await previewBlocks()).map((b) => b.t));
      const stable = n !== undefined && n === last;
      last = n;
      if (!stable) await new Promise((r) => setTimeout(r, 300));
      before = n;
      return stable;
    },
    10_000,
    "a settled position",
  );
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

test("the outline rail lists the headings, follows the position and goes to a heading", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, KEYS.shift, "o"]);
  const n = await untilOrDump(
    () =>
      wd.exec(`
        const rail = document.querySelector('.md-outline');
        if (!rail || rail.offsetParent === null) return false;
        const k = window.__quire.session.md.outline.headings().length;
        return k > 0 && document.querySelectorAll('.md-outline-row').length > 0 && k;
      `),
    10_000,
    "outline rows",
  );
  // 400 "## Section i" ("# Notes title" became "Hello # Notes title" in
  // the typing flow).
  assert.equal(n, 400);
  // Go to a heading far down: the editing view scrolls there, and it is
  // the current one.
  await wd.exec(`
    const list = document.querySelector('.md-outline-list');
    list.scrollTop = 201 * 24 - 48;
  `);
  await wd.until(
    () =>
      wd.exec(
        "return [...document.querySelectorAll('.md-outline-row')].some((r) => r.textContent === 'Section 200')",
      ),
    5000,
    "row for Section 200",
  );
  await wd.exec(
    "[...document.querySelectorAll('.md-outline-row')].find((r) => r.textContent === 'Section 200').click()",
  );
  await untilOrDump(
    async () => firstSection((await sourceRows()).map((r) => r.t)) === 200,
    5000,
    "source at Section 200",
  );
  await wd.until(
    () =>
      wd
        .exec("return document.querySelector('.md-outline-row.current')?.textContent ?? ''")
        .then((t) => t === "Section 200"),
    5000,
    "Section 200 is current",
  );
  // The choice is remembered; turn it off again for the next flows.
  assert.equal(await wd.exec("return localStorage.getItem('quire.md.outline')"), "on");
  await focusEditor();
  await wd.chord([KEYS.control, KEYS.shift, "o"]);
  await wd.until(
    () => wd.exec("return document.querySelector('.md-outline').offsetParent === null"),
    5000,
    "outline hidden",
  );
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

/** The document's text, as the core has it (a Buffer: byte offsets). */
async function docText() {
  await focusEditor();
  await wd.chord([KEYS.control, "s"]);
  await wd.until(async () => /Saved/.test(await status()), 10_000, "saved");
  return fs.readFileSync(sample);
}

test("table helpers: format, Tab between cells, one undo step each", async () => {
  const before = await docText();
  const at = before.indexOf(Buffer.from("| a | b |"));
  assert.ok(at >= 0);
  await focusEditor();
  await wd.exec(
    `const v = window.__quire.session.view; v.send({ type: "setSelections", ranges: [[${at + 2}, ${at + 2}]] }); void v.reveal(${at});`,
  );
  await wd.until(
    () =>
      wd.exec(
        "return [...document.querySelectorAll('.statusbar button')].some((b) => b.textContent === 'Table')",
      ),
    5000,
    "caret in the table",
  );
  await wd.chord([KEYS.alt, KEYS.shift, "f"]);
  let text = await untilOrDump(
    async () => {
      const t = await docText();
      return t.includes("| a   | b   |\n| --- | --- |\n| 1   | 2   |") && t;
    },
    10_000,
    "formatted table",
  );
  // Only the table changed.
  const end = before.indexOf(Buffer.from("| 1 | 2 |")) + "| 1 | 2 |".length;
  assert.deepEqual(text.subarray(0, at), before.subarray(0, at));
  assert.deepEqual(
    text.subarray(text.indexOf(Buffer.from("| 1   | 2   |")) + 13),
    before.subarray(end),
  );
  // Tab from the last cell adds a row; Shift+Tab goes back.
  const two = text.indexOf(Buffer.from("| 2   |")) + 3;
  await wd.exec(
    `window.__quire.session.view.send({ type: "setSelections", ranges: [[${two}, ${two}]] })`,
  );
  await new Promise((r) => setTimeout(r, 200));
  await wd.chord([KEYS.tab]);
  await untilOrDump(
    async () => {
      const t = await docText();
      return t.includes("| 1   | 2   |\n|     |     |") && t;
    },
    10_000,
    "a new row",
  );
  await wd.exec(
    `document.querySelector('.txt-input').dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertText', data: 'z', cancelable: true }))`,
  );
  await untilOrDump(
    async () => {
      const t = await docText();
      return /\| z +\| +\|/.test(t.toString());
    },
    10_000,
    "typed in the new row's first cell",
  );
  // Undo: the typing, then the new row (one step), then the format (one step).
  await focusEditor();
  for (let i = 0; i < 3; i++) await wd.chord([KEYS.control, "z"]);
  await untilOrDump(
    async () => {
      const t = await docText();
      return t.equals(before) && t;
    },
    10_000,
    "back to the original bytes",
  );
});

test("pasting an image stores it in assets/ once and links it (one undo step)", async () => {
  const before = await docText();
  const png3 = png(3, 2);
  const paste = `
    const bytes = new Uint8Array([${[...png3].join(",")}]);
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "image.png", { type: "image/png" }));
    document.querySelector('.txt-input').dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
  `;
  await focusEditor();
  await wd.exec(`window.__quire.session.view.send({ type: "setSelections", ranges: [[0, 0]] })`);
  await new Promise((r) => setTimeout(r, 200));
  await wd.exec(paste);
  // Wait for the link in the view before saving: an edit that arrives
  // while a save runs is refused ("busy").
  await untilOrDump(
    async () => (await sourceRows()).some((r) => r.t.startsWith("![](assets/image-")),
    10_000,
    "the link in the source view",
  );
  const text = await untilOrDump(
    async () => {
      const t = await docText();
      globalThis.__head = t.subarray(0, 60).toString();
      return t.subarray(0, 4).toString() === "![](" && t;
    },
    10_000,
    "the link inserted",
  ).catch(async (err) => {
    throw new Error(
      `${String(err)} head=${JSON.stringify(globalThis.__head)} md=${JSON.stringify(await wd.exec("return { rev: window.__quire.session.view.primary(), sel: window.__quire.session.view.primary() }"))}`,
    );
  });
  const link = /^!\[\]\((assets\/image-\d{8}-[0-9a-f]{8}\.png)\)/.exec(text.toString())[1];
  assert.deepEqual(fs.readFileSync(path.join(mdDir, link)), png3);
  // The same image again: the same file.
  await wd.exec(paste);
  await untilOrDump(
    async () =>
      (await sourceRows()).some((r) => r.t.startsWith("![](assets/") && r.t.indexOf("![](", 4) > 0),
    10_000,
    "the second link in the source view",
  );
  await untilOrDump(
    async () => (await docText()).toString().startsWith(`![](${link})![](${link})`),
    10_000,
    "the same link again",
  );
  assert.equal(fs.readdirSync(path.join(mdDir, "assets")).length, 1);
  await focusEditor();
  for (let i = 0; i < 2; i++) await wd.chord([KEYS.control, "z"]);
  await untilOrDump(async () => (await docText()).equals(before), 10_000, "undone");
});
