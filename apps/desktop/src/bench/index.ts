// Benchmark entry point; only bundled when built with `--mode bench`
// (VITE_QUIRE_BENCH=1) and only usable with the `bench-fixtures` cargo feature.

import { invoke } from "@tauri-apps/api/core";

type Scenario = { run(host: HTMLElement): Promise<unknown> };

const scenarios: Record<string, () => Promise<Scenario>> = {
  startup: () => import("./startup"),
  "scroll-list": () => import("./scroll-list"),
  hud: () => import("./hud"),
};

/**
 * Runs the scenario named by QUIRE_BENCH, if any, in a full-window layer
 * above the app. Called after the app's first frame, so startup timings
 * measure the real UI. Returns whether a scenario ran.
 */
export async function runBenchIfRequested(): Promise<boolean> {
  const name = await invoke<string | null>("bench_scenario").catch(() => null);
  if (!name) return false;
  const host = document.createElement("div");
  host.className = "bench-host";
  document.body.append(host);
  const load = scenarios[name];
  let results: unknown;
  try {
    if (!load) throw new Error(`unknown scenario ${name}`);
    results = await (await load()).run(host);
  } catch (err) {
    results = { scenario: name, error: String(err) };
  }
  await invoke("bench_report", { results });
  return true;
}
