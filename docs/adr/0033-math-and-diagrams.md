# 0033. Math and diagrams in the preview: KaTeX in the page, Mermaid in a sandboxed frame

- Status: Accepted
- Date: 2026-10-15

## Context

§3.1: KaTeX math and Mermaid in the Markdown view. §6.2: render KaTeX and Mermaid lazily, Mermaid in an isolated offscreen context, with the source and an error badge on failure. §7: progressive everything. §8: every file is hostile; no network; strict CSP. §2: no UI-thread task over 8 ms. Both libraries are JavaScript; the preview's blocks are rendered in the core and shown as DOM (ADR-0030); comrak marks math as `data-math-style` elements, and Mermaid is a fenced code block (` ```mermaid `).

## Decision

**Lazy, after the paint, near the viewport** (`apps/desktop/src/md/rich.ts`): as the preview places blocks near the viewport (where it loads images), it queues their math and diagrams once per rendering. Until rendered, the TeX source shows muted and a diagram shows as its code; neither holds the first paint.

**KaTeX in the page**: bundled (the module, its CSS and fonts as local assets; no network), loaded on first use as its own chunk. Expressions render in slices of at most 4 ms per task, cached by (source, display) up to 2,000 entries. `trust: false` (no `\href`, `\url`, `\htmlClass`…), `maxExpand` 1,000, `maxSize` 100 em; output HTML plus MathML (accessible). Sources over 10 KiB aren't rendered (labeled). On an error the source stays, outlined, with a "Math error" badge whose tooltip is KaTeX's message. ` ```math ` blocks render as display math.

**Mermaid in a sandboxed frame** (`apps/desktop/src/mermaid-frame.ts`, `mermaid-frame.html`):
- An offscreen `<iframe sandbox="allow-scripts">`: an opaque origin, so no IPC (Tauri rejects it), no storage, no access to the app's document, no navigation, popups or forms. It is created on the first diagram and kept.
- Mermaid needs inline styles to measure its text, which the app's CSP forbids, so the page is served by the host's `quire-frame` scheme (`src-tauri/src/frame.rs`) with a policy of its own: scripts, styles and fonts from that scheme (plus inline styles), images from `data:`, nothing else, and only the app may frame it. The scheme serves only the frame page and the built assets, and takes out the app-wide CSP `<meta>` that Tauri's build adds to every HTML file (inside the frame, `'self'` of an opaque origin would block its own scripts). The app's CSP gains `frame-src` for that scheme only.
- Tauri's `freezePrototype` (`Object.freeze(Object.prototype)` as an initialization script) runs in subframes too on Windows, although Tauri marks it main-frame-only, and Mermaid's dependencies can't load with it ("Cannot assign to read only property 'valueOf'", found by the UI flow). The setting is off, and the app's own entry freezes the prototype as its first module (`src/freeze.ts`), before any dependency runs; a UI flow checks it stays frozen.
- One diagram at a time, `securityLevel: "strict"`, text as SVG text (no HTML labels), `maxTextSize` 50,000, `maxEdges` 2,000, the theme following the system's. The frame answers with the SVG as a string, or the error; replies are checked field by field (untrusted data).
- **The SVG is shown as an image** (`<img src="data:image/svg+xml,…">`): whatever Mermaid produced, it can't run script, load anything or style the app's page. Cached by (source, theme), 500 entries.
- On an error, the code stays, outlined, with a "Diagram error" badge (Mermaid's message as tooltip); over 50 KiB, "too large to render"; a diagram that takes over 10 s, or a frame that doesn't start within 10 s, ends with an error badge too, never a spinner forever.

Both caches report their size to the memory governor (`md-rich`, through `memory_report` like the preview's other webview caches) and are dropped on a `memory-trim`.

## Alternatives

- *KaTeX in the core* (a Rust port, or KaTeX through a JS engine in the core): no maintained Rust port with KaTeX's coverage; a JS engine in the core for one library is out of proportion.
- *Mermaid in the page* with `securityLevel: "strict"`: one bug in Mermaid or its dependencies would run in the document that holds the IPC bridge. §6.2 asks for isolation.
- *Mermaid in a Web Worker*: it needs a DOM to lay out text.
- *A `srcdoc` frame*: it inherits the app's CSP (no inline styles, so Mermaid can't measure its text).
- *Inserting the SVG into the page*: needs another sanitizer pass in the frontend and keeps Mermaid's styles in the page's scope; an image needs neither.

## Consequences

- The diagram is an image: its text isn't selectable or found by the preview's find (the source is, in the source view). Clicking it still goes to its source (it keeps `data-sourcepos`).
- Whether the sandboxed frame runs on its own thread is the browser's choice (Chromium's sandboxed-iframe process isolation); if it shares the page's thread, a large diagram's layout shows as a long task. The md-20m scenarios include Mermaid blocks; the ref-sim results show whether it does.
- Licenses: KaTeX (MIT); Mermaid 11.17.2 and its tree (MIT, ISC, BSD, Apache-2.0, MPL-2.0 OR Apache-2.0, and `robust-predicates`, Unlicense: ADR-0032).
