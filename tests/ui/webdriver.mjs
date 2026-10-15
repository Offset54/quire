// A minimal W3C WebDriver client (ADR-0020: UI-flow tests drive the real
// app through tauri-driver + msedgedriver). Only what the flows use.

const KEYS = {
  enter: "",
  escape: "",
  backspace: "",
  pageDown: "",
  pageUp: "",
  end: "",
  home: "",
  left: "",
  up: "",
  right: "",
  down: "",
  control: "",
  shift: "",
  alt: "",
  tab: "",
  f3: "",
};
export { KEYS };

export class WebDriver {
  constructor(base = "http://127.0.0.1:4444") {
    this.base = base;
    this.session = undefined;
  }

  async cmd(method, path, body) {
    const url = `${this.base}${this.session ? `/session/${this.session}` : ""}${path}`;
    const res = await fetch(url, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const json = await res.json();
    if (!res.ok || json.value?.error) {
      throw new Error(
        `${method} ${path}: ${json.value?.error ?? res.status} ${json.value?.message ?? ""}`,
      );
    }
    return json.value;
  }

  async start(application, args = []) {
    const v = await this.cmd("POST", "/session", {
      capabilities: {
        alwaysMatch: { "tauri:options": { application, args } },
      },
    });
    this.session = v.sessionId;
  }

  async stop() {
    if (!this.session) return;
    try {
      await this.cmd("DELETE", "");
    } finally {
      this.session = undefined;
    }
  }

  async find(css) {
    const v = await this.cmd("POST", "/element", { using: "css selector", value: css });
    return Object.values(v)[0];
  }

  async findAll(css) {
    const v = await this.cmd("POST", "/elements", { using: "css selector", value: css });
    return v.map((e) => Object.values(e)[0]);
  }

  async click(el) {
    await this.cmd("POST", `/element/${el}/click`, {});
  }

  async text(el) {
    return this.cmd("GET", `/element/${el}/text`);
  }

  async type(el, text) {
    await this.cmd("POST", `/element/${el}/value`, { text });
  }

  /** Key chord through the actions API, e.g. chord([KEYS.control, "z"]). */
  async chord(keys) {
    const down = keys.map((k) => ({ type: "keyDown", value: k }));
    const up = [...keys].reverse().map((k) => ({ type: "keyUp", value: k }));
    await this.cmd("POST", "/actions", {
      actions: [{ type: "key", id: "kb", actions: [...down, ...up] }],
    });
    await this.cmd("DELETE", "/actions");
  }

  async exec(script, args = []) {
    return this.cmd("POST", "/execute/sync", { script, args });
  }

  async execAsync(script, args = []) {
    return this.cmd("POST", "/execute/async", { script, args });
  }

  /** Polls `fn` until it returns a truthy value or the timeout passes. */
  async until(fn, timeoutMs = 10_000, what = "condition") {
    const start = Date.now();
    let last;
    while (Date.now() - start < timeoutMs) {
      try {
        last = await fn();
        if (last) return last;
      } catch (err) {
        last = err;
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    throw new Error(`timed out waiting for ${what} (last: ${String(last)})`);
  }
}
