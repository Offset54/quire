// Checks that the performance HUD shows live data while a surface scrolls.

import { invoke } from "@tauri-apps/api/core";
import { DocumentSurface, type ViewInfo } from "../surface/surface";

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function run(host: HTMLElement): Promise<unknown> {
  const surface = new DocumentSurface(host);
  const { width, dpr } = surface.viewport;
  const info = await invoke<ViewInfo>("bench_open_synthetic", {
    rows: 1_000_000,
    rowHeight: 20 * dpr,
    width,
    channel: surface.createChannel(),
  });
  surface.attach(info);
  const toggle = () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "H", ctrlKey: true, shiftKey: true }));
  };
  toggle();
  const start = performance.now();
  while (performance.now() - start < 2000) {
    surface.scrollBy(40 * dpr);
    await nextFrame();
  }
  await sleep(600); // at least one stats poll after scrolling
  const hud = document.querySelector(".perf-hud");
  const text = hud?.querySelector("pre")?.textContent ?? null;
  const graph = hud?.querySelector("canvas");
  const painted = graph
    ? graph
        .getContext("2d")!
        .getImageData(0, 0, graph.width, graph.height)
        .data.some((v, i) => i % 4 === 3 && v > 0)
    : false;
  toggle();
  return {
    scenario: "hud",
    visibleWhileOpen: text !== null,
    graphPainted: painted,
    hiddenAfterToggle: !document.querySelector(".perf-hud"),
    text,
  };
}
