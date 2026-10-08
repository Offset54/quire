import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { setLocale } from "./i18n";
import { startTracing } from "./perf/trace";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");

setLocale(navigator.language);
void startTracing();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// Startup metric: navigation start to the first painted frame (§4 cold start).
requestAnimationFrame(() => {
  setTimeout(() => performance.measure("first-frame", { start: 0, end: performance.now() }), 0);
});
