// Compares two benchmark result files and fails on regressions (§10: CI
// fails on any regression over 10 %).
//
//   node tools/bench/compare.mjs refsim <base.json> <head.json> [--threshold 10] [--min-abs 0.5]
//   node tools/bench/compare.mjs criterion <baseline-name> <head-name> [--threshold 10]
//
// refsim: compares every budget metric (lower is better). Changes smaller than
// --min-abs (in the metric's unit) are ignored as noise.
// criterion: compares the median estimate of every benchmark under
// target/criterion between two saved baselines.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const [mode, base, head, ...rest] = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = rest.indexOf(name);
  return i >= 0 ? Number(rest[i + 1]) : fallback;
};
const threshold = opt("--threshold", 10);
const minAbs = opt("--min-abs", 0.5);

/** @returns {{name: string, base: number, head: number, unit: string}[]} */
function refsimPairs() {
  const a = JSON.parse(readFileSync(base, "utf8"));
  const b = JSON.parse(readFileSync(head, "utf8"));
  const before = new Map(a.budgets.map((r) => [r.id, r]));
  return b.budgets
    .filter((r) => before.has(r.id))
    .map((r) => ({ name: r.id, base: before.get(r.id).value, head: r.value, unit: r.unit }));
}

function criterionEstimates(baseline) {
  const root = "target/criterion";
  const out = new Map();
  const walk = (dir, name) => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (!statSync(path).isDirectory()) continue;
      if (entry === baseline) {
        const est = join(path, "estimates.json");
        if (existsSync(est))
          out.set(name, JSON.parse(readFileSync(est, "utf8")).median.point_estimate);
      } else if (entry !== "report") {
        walk(path, name ? `${name}/${entry}` : entry);
      }
    }
  };
  if (existsSync(root)) walk(root, "");
  return out;
}

function criterionPairs() {
  const a = criterionEstimates(base);
  const b = criterionEstimates(head);
  return [...b]
    .filter(([n]) => a.has(n))
    .map(([n, v]) => ({ name: n, base: a.get(n), head: v, unit: "ns" }));
}

if (!["refsim", "criterion"].includes(mode) || !base || !head) {
  console.error(
    "usage: compare.mjs refsim|criterion <base> <head> [--threshold 10] [--min-abs 0.5]",
  );
  process.exit(2);
}
const pairs = mode === "refsim" ? refsimPairs() : criterionPairs();
const floor = mode === "refsim" ? minAbs : 0;
let regressions = 0;
for (const p of pairs) {
  const change = p.base === 0 ? (p.head === 0 ? 0 : Infinity) : ((p.head - p.base) / p.base) * 100;
  const regressed = change > threshold && p.head - p.base > floor;
  if (regressed) regressions++;
  console.log(
    `${regressed ? "REGRESSED" : "ok       "} ${p.name.padEnd(40)} ${p.base.toFixed(2)} -> ${p.head.toFixed(2)} ${p.unit} (${change >= 0 ? "+" : ""}${change.toFixed(1)} %)`,
  );
}
if (pairs.length === 0) console.log("nothing to compare");
if (regressions > 0) {
  console.error(`${regressions} regression(s) over ${threshold} %`);
  process.exit(1);
}
