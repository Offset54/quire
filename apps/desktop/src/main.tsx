import "./freeze";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { startFontWorker, warmFonts } from "./fonts";
import { setLocale } from "./i18n";
import { installHud } from "./perf/hud";
import { startTracing } from "./perf/trace";

// A file from the command line is opening (the host says so, ADR-0022):
// warm the fonts now, before its first rows. Otherwise after the first frame,
// which the worker would slow (RISKS #13).
if ((globalThis as { __quireOpening?: boolean }).__quireOpening) startFontWorker();

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");

setLocale(navigator.language);
void startTracing();
installHud();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// Startup metric: navigation start to the first painted frame (§4 cold start).
requestAnimationFrame(() => {
  setTimeout(() => {
    performance.measure("first-frame", { start: 0, end: performance.now() });
    warmFonts();
    // Dead code in normal builds: Vite replaces the condition with `false`.
    if (import.meta.env.VITE_QUIRE_BENCH === "1") {
      void import("./bench").then((b) => b.runBenchIfRequested());
    }
  }, 0);
});
