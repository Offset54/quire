// Frame stalls after idle in plain Microsoft Edge (no quire code): a page
// with text rows scrolls for 250 ms after each idle period and reports the
// worst frame interval of each (M1: platform frame stalls, RISKS #12).
// `keepalive` adds a 1 px compositor animation that keeps frames flowing.
// Usage: start msedgedriver (tools/webdriver/fetch-msedgedriver.ps1) with
// --port=4445, then: node tools/bench/edge-idle-probe.mjs [keepalive]
const base = "http://127.0.0.1:4445";
async function cmd(session, method, path, body) {
  const url = `${base}${session ? `/session/${session}` : ""}${path}`;
  const res = await fetch(url, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = await res.json();
  if (!res.ok || json.value?.error) throw new Error(JSON.stringify(json.value));
  return json.value;
}
const v = await cmd(undefined, "POST", "/session", {
  capabilities: {
    alwaysMatch: {
      browserName: "MicrosoftEdge",
      "ms:edgeOptions": { args: ["--window-size=1280,800"] },
    },
  },
});
const s = v.sessionId;
try {
  await cmd(s, "POST", "/url", { url: "about:blank" });
  const script = `const KEEPALIVE = ${process.argv[2] === "keepalive"};
    const done = arguments[0];
    document.body.style.margin = "0";
    const layer = document.createElement("div");
    layer.style.cssText = "position:absolute;left:0;top:0;will-change:transform;font:14px monospace";
    let html = "";
    for (let i = 0; i < 80; i++) html += "<div>" + i + " the quick brown fox 한국어 日本語 🙂 jumps over the lazy dog " + "x".repeat(i % 40) + "</div>";
    layer.innerHTML = html;
    document.body.append(layer);
    if (KEEPALIVE) { const st = document.createElement("style"); st.textContent = "@keyframes ka{from{transform:rotate(0)}to{transform:rotate(360deg)}}"; document.head.append(st); const k = document.createElement("div"); k.style.cssText = "position:fixed;right:0;bottom:0;width:1px;height:1px;background:#fff;animation:ka 1s linear infinite"; document.body.append(k); }
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const frame = () => new Promise((r) => requestAnimationFrame(r));
    (async () => {
      const out = [];
      let y = 0;
      for (const idle of [100, 300, 500, 700, 1000, 2000, 300, 500, 700, 300, 500]) {
        await sleep(idle);
        let last = await frame();
        const start = last;
        let worst = 0;
        while (last - start < 250) {
          const t = await frame();
          worst = Math.max(worst, t - last);
          y = (y + 2) % 400;
          layer.style.transform = "translateY(" + (-y) + "px)";
          last = t;
        }
        out.push([idle, Math.round(worst * 10) / 10]);
      }
      done(JSON.stringify(out));
    })();
  `;
  const r = await cmd(s, "POST", "/execute/async", { script, args: [] });
  console.log(r);
} finally {
  await cmd(s, "DELETE", "");
}
