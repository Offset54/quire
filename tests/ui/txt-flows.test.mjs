// TXT editor UI flows, driven through the real app with WebDriver
// (ADR-0020). Run with `pwsh tools/webdriver/run-ui-tests.ps1`, which builds
// the app, fetches the matching msedgedriver and starts tauri-driver.
//
// QUIRE_APP: path to quire.exe. QUIRE_UI_DIR: a scratch directory; the app's
// data (journals, WebView2 profile) lives there too, through LOCALAPPDATA.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { after, before, test } from "node:test";
import { KEYS, WebDriver } from "./webdriver.mjs";

const APP = process.env.QUIRE_APP;
const DIR = process.env.QUIRE_UI_DIR;
if (!APP || !DIR) throw new Error("QUIRE_APP and QUIRE_UI_DIR must be set");

const wd = new WebDriver();
const sample = path.join(DIR, "sample.txt");

function writeSample() {
  const lines = [];
  for (let i = 0; i < 3000; i++) {
    if (i % 100 === 7) lines.push(`한국어 줄 ${i}: 대한민국은 민주공화국이다.`);
    else lines.push(`line ${i}\tsome text with a tab`);
  }
  fs.writeFileSync(sample, lines.join("\r\n") + "\r\n");
}

/** Text of the rows on screen, top to bottom. */
function rows() {
  return wd.exec(`
    const vp = document.querySelector('.txt-viewport').getBoundingClientRect();
    return [...document.querySelectorAll('.txt-row:not(.txt-probe)')]
      .filter((r) => r.style.display !== 'none')
      .map((r) => ({ y: r.getBoundingClientRect().top, t: r.querySelector('.txt-text').textContent, ln: r.querySelector('.txt-ln').textContent }))
      .filter((r) => r.y >= vp.top - 1 && r.y < vp.bottom)
      .sort((a, b) => a.y - b.y);
  `);
}

const status = () => wd.exec("return document.querySelector('.statusbar')?.innerText ?? ''");
const focusEditor = () => wd.exec("document.querySelector('.txt-input').focus()");

/** Types text through the keyboard (key events on the focused element). */
async function keys(text) {
  const actions = [];
  for (const ch of text) actions.push({ type: "keyDown", value: ch }, { type: "keyUp", value: ch });
  await wd.cmd("POST", "/actions", { actions: [{ type: "key", id: "kb", actions }] });
  await wd.cmd("DELETE", "/actions");
}

async function launch(args) {
  await wd.start(APP, args);
  await wd.until(
    async () =>
      (await wd.exec("return !!document.querySelector('.welcome')")) || (await rows()).length > 5,
    20_000,
    "first content",
  );
}

before(() => {
  fs.mkdirSync(DIR, { recursive: true });
  writeSample();
});

after(async () => {
  await wd.stop();
});

test("opens a file from the command line and shows its first lines", async () => {
  await launch([sample]);
  const r = await rows();
  assert.equal(r[0].t, "line 0\tsome text with a tab");
  assert.equal(r[0].ln, "1");
  assert.match(r[7].t, /^한국어 줄 7/);
  await wd.until(async () => /3,001 lines/.test(await status()), 10_000, "exact line count");
  assert.match(await status(), /CRLF/);
  assert.match(await status(), /UTF-8/);
  assert.match(await status(), /Saved/);
});

test("typing, undo and redo", async () => {
  await focusEditor();
  await keys("Hello ");
  await wd.until(async () => (await rows())[0].t.startsWith("Hello line 0"), 5000, "typed text");
  assert.match(await status(), /Unsaved/);
  await wd.chord([KEYS.control, "z"]);
  await wd.until(async () => (await rows())[0].t === "line 0\tsome text with a tab", 5000, "undo");
  await wd.chord([KEYS.control, "y"]);
  await wd.until(async () => (await rows())[0].t.startsWith("Hello "), 5000, "redo");
});

test("IME composition commits once (Korean)", async () => {
  // Synthetic composition events, as the Microsoft IME sends them.
  await wd.exec(`
    const ta = document.querySelector('.txt-input');
    ta.dispatchEvent(new CompositionEvent('compositionstart', { data: '' }));
    ta.dispatchEvent(new CompositionEvent('compositionupdate', { data: 'ㅎ' }));
    ta.dispatchEvent(new CompositionEvent('compositionupdate', { data: '하' }));
    ta.dispatchEvent(new CompositionEvent('compositionupdate', { data: '한' }));
    ta.dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertCompositionText', data: '한', isComposing: true, cancelable: true }));
    ta.dispatchEvent(new CompositionEvent('compositionend', { data: '한' }));
    ta.dispatchEvent(new InputEvent('input', { inputType: 'insertCompositionText', data: '한', isComposing: false }));
  `);
  await wd.until(
    async () => (await rows())[0].t.startsWith("Hello 한line 0"),
    5000,
    "committed composition",
  );
  const r = await rows();
  assert.equal((r[0].t.match(/한/g) ?? []).length, 1, "no duplicated syllable");
});

test("a burst of typing keeps every character, in order", async () => {
  // Faster than the core answers: drawn as local echoes, queued and
  // coalesced, then confirmed. Backspaces queue behind them.
  const burst = "the quick brown fox 0123456789 jumps!?";
  await wd.exec(`
    const ta = document.querySelector('.txt-input');
    for (const ch of ${JSON.stringify(burst)}) {
      ta.dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertText', data: ch, cancelable: true }));
    }
  `);
  await wd.until(
    async () => (await rows())[0].t === `Hello 한${burst}line 0\tsome text with a tab`,
    5000,
    "burst typed",
  );
  await wd.exec(`
    const ta = document.querySelector('.txt-input');
    for (let i = 0; i < ${burst.length}; i++) {
      ta.dispatchEvent(new InputEvent('beforeinput', { inputType: 'deleteContentBackward', cancelable: true }));
    }
  `);
  await wd.until(
    async () => (await rows())[0].t === "Hello 한line 0\tsome text with a tab",
    5000,
    "burst deleted",
  );
});

test("go to line", async () => {
  await wd.chord([KEYS.control, "g"]);
  const input = await wd.find(".goto input");
  await wd.type(input, "2500" + KEYS.enter);
  await wd.until(
    async () => (await rows()).some((r) => r.t.startsWith("line 2499\t") && r.ln === "2500"),
    5000,
    "line 2500 on screen",
  );
});

test("find counts matches and replace-all is one undo step", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "f"]);
  const find = await wd.find(".find input");
  await wd.type(find, "한국어");
  await wd.until(
    async () =>
      /30 matches/.test(await wd.exec("return document.querySelector('.find-count').innerText")),
    10_000,
    "match count",
  );
  await wd.chord([KEYS.control, "h"]);
  const inputs = await wd.findAll(".find input");
  await wd.type(inputs[1], "KOREAN");
  const buttons = await wd.findAll(".find button");
  const labels = await Promise.all(buttons.map((b) => wd.text(b)));
  await wd.click(buttons[labels.indexOf("Replace all")]);
  await wd.until(
    async () =>
      /30 replacements/.test(await wd.exec("return document.querySelector('.notices').innerText")),
    10_000,
    "replace-all notice",
  );
  await focusEditor();
  await wd.chord([KEYS.control, "z"]);
  await wd.until(
    async () =>
      /30 matches/.test(await wd.exec("return document.querySelector('.find-count').innerText")),
    10_000,
    "undone in one step",
  );
  await wd.chord([KEYS.control, "y"]);
  await wd.until(
    async () =>
      /0 matches/.test(await wd.exec("return document.querySelector('.find-count').innerText")),
    10_000,
    "redone",
  );
  await wd.exec("document.querySelector('.find button[title=Close]')?.click()");
});

test("save writes the edits and keeps CRLF endings", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, "s"]);
  await wd.until(async () => /Saved/.test(await status()), 10_000, "saved");
  const disk = fs.readFileSync(sample, "utf8");
  assert.ok(disk.startsWith("Hello 한line 0\tsome text with a tab\r\n"));
  assert.ok(disk.includes("KOREAN 줄 7: "));
  assert.ok(!/[^\r]\n/.test(disk), "every break is still CRLF");
});

test("a killed app offers to restore its unsaved edits", async () => {
  await focusEditor();
  await wd.chord([KEYS.control, KEYS.home]);
  await keys("UNSAVED ");
  await wd.until(async () => /Unsaved/.test(await status()), 5000, "dirty");
  // Kill the app without letting it close (journal writes happen at once).
  await new Promise((r) => setTimeout(r, 300));
  try {
    execFileSync("taskkill", ["/F", "/IM", "quire.exe"]);
  } catch {
    // already gone
  }
  await wd.stop().catch(() => {});
  await launch([]);
  await wd.until(
    () => wd.exec("return !!document.querySelector('.recovery')"),
    10_000,
    "recovery offer",
  );
  await wd.exec(
    "[...document.querySelectorAll('.recovery button')].find((b) => b.innerText === 'Restore').click()",
  );
  await wd.until(
    async () => ((await rows())[0]?.t ?? "").startsWith("UNSAVED Hello "),
    10_000,
    "restored edit",
  );
  assert.match(await status(), /Unsaved/);
});
test("a large non-UTF-8 file shows decode progress and can be cancelled", async () => {
  // 768 MiB of UTF-16LE: decoding takes long enough to see and cancel.
  const big = path.join(DIR, "big-utf16.txt");
  const line = Buffer.from("UTF-16 line, 한국어 and 日本語, to decode\r\n", "utf16le");
  const block = Buffer.concat(
    Array.from({ length: Math.ceil((16 << 20) / line.length) }, () => line),
  );
  const fd = fs.openSync(big, "w");
  fs.writeSync(fd, Buffer.from([0xff, 0xfe]));
  for (let i = 0; i < 48; i++) fs.writeSync(fd, block);
  fs.closeSync(fd);
  await wd.stop().catch(() => {});
  await launch([big]);
  await wd.until(
    () => wd.exec("return /Decoding/.test(document.querySelector('.banner')?.innerText ?? '')"),
    10_000,
    "decode progress",
  );
  await wd.exec(
    "[...document.querySelectorAll('.banner button')].find((b) => b.innerText === 'Cancel').click()",
  );
  await wd.until(
    () =>
      wd.exec("return !document.querySelector('.banner') && !!document.querySelector('.welcome')"),
    10_000,
    "back to the welcome screen",
  );
  assert.equal(await wd.exec("return document.querySelectorAll('.txt-row').length"), 0);
  assert.equal(await wd.exec("return document.querySelectorAll('.notice.error').length"), 0);
  fs.rmSync(big);
});
