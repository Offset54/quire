// The Mermaid frame (ADR-0033): runs in a sandboxed iframe (opaque origin,
// no IPC, its own CSP from the host's `quire-frame` scheme). It renders one
// diagram per message and answers with the SVG or the error; the app shows
// the SVG as an image, so nothing in it runs in the app's document.

export interface FrameRequest {
  id: number;
  source: string;
  dark: boolean;
}

export type FrameReply =
  | { ready: true }
  | { failed: string }
  | { id: number; svg: string }
  | { id: number; error: string };

type Mermaid = (typeof import("mermaid"))["default"];

let theme: "default" | "dark" | undefined;

function configure(mermaid: Mermaid, dark: boolean): void {
  const want = dark ? "dark" : "default";
  if (theme === want) return;
  theme = want;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: want,
    // Text as SVG text, not HTML in foreignObject: the result is shown as an
    // image, without the page's fonts or styles.
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    maxTextSize: 50_000,
    maxEdges: 2_000,
  });
}

function reply(r: FrameReply): void {
  window.parent.postMessage(r, "*");
}

const message = (err: unknown) => (err instanceof Error ? err.message : String(err));

// Loaded here, not imported at the top, so a failure to load is reported
// to the app instead of leaving it waiting.
import("mermaid").then(
  ({ default: mermaid }) => {
    let queue = Promise.resolve();
    window.addEventListener("message", (e: MessageEvent<FrameRequest>) => {
      if (e.source !== window.parent) return;
      const { id, source, dark } = e.data;
      if (typeof id !== "number" || typeof source !== "string") return;
      // One diagram at a time: Mermaid renders through shared DOM state.
      queue = queue.then(async () => {
        try {
          configure(mermaid, dark);
          const { svg } = await mermaid.render(`d${String(id)}`, source);
          reply({ id, svg });
        } catch (err) {
          reply({ id, error: message(err) });
        } finally {
          document.getElementById(`d${String(id)}`)?.remove();
          document.getElementById(`dd${String(id)}`)?.remove();
        }
      });
    });
    reply({ ready: true });
  },
  (err: unknown) => {
    reply({ failed: message(err) });
  },
);
