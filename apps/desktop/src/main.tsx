import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { setLocale } from "./i18n";
import { startTracing } from "./perf/trace";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");

setLocale(navigator.language);
void startTracing();

async function start(host: HTMLElement) {
  // Dead code in normal builds: Vite replaces the condition with `false`.
  if (import.meta.env.VITE_QUIRE_BENCH === "1") {
    const { runBenchIfRequested } = await import("./bench");
    if (await runBenchIfRequested(host)) return;
  }
  createRoot(host).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
  // Startup metric: navigation start to the first painted frame (§4 cold start).
  requestAnimationFrame(() => {
    setTimeout(() => performance.measure("first-frame", { start: 0, end: performance.now() }), 0);
  });
}

void start(root);
