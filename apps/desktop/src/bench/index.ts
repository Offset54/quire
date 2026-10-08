// Benchmark entry point; only bundled when built with `--mode bench`
// (VITE_QUIRE_BENCH=1) and only usable with the `bench-fixtures` cargo feature.

import { invoke } from "@tauri-apps/api/core";

type Scenario = { run(host: HTMLElement): Promise<unknown> };

const scenarios: Record<string, () => Promise<Scenario>> = {
  "scroll-list": () => import("./scroll-list"),
  hud: () => import("./hud"),
};

/** Runs the scenario named by QUIRE_BENCH, if any. Returns whether one ran. */
export async function runBenchIfRequested(host: HTMLElement): Promise<boolean> {
  const name = await invoke<string | null>("bench_scenario").catch(() => null);
  if (!name) return false;
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
