// Fails if any production (shipped) JS dependency has a license outside the
// allowlist (MASTER_PROMPT §0, ADR-0019). Dev-only tools don't ship and are
// not gated. Keep ALLOWED in sync with deny.toml.
import { execSync } from "node:child_process";

const ALLOWED = new Set([
  "MIT",
  "MIT-0",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Zlib",
  "MPL-2.0",
  "Unicode-3.0",
  "Unicode-DFS-2016",
  "BSL-1.0",
  "CC0-1.0",
  "0BSD",
]);

/** Evaluates a simple SPDX expression: ids joined by OR / AND, optional parentheses. */
export function isAllowed(expr) {
  const s = expr.trim().replace(/^\((.*)\)$/, "$1");
  if (/\sOR\s/.test(s)) return s.split(/\s+OR\s+/).some(isAllowed);
  if (/\sAND\s/.test(s)) return s.split(/\s+AND\s+/).every(isAllowed);
  return ALLOWED.has(s);
}

// A fixed command string: on Windows pnpm is a .cmd shim and needs a shell.
const json = execSync("pnpm -r licenses list --prod --json", { encoding: "utf8" });
const byLicense = JSON.parse(json);

const rejected = [];
let count = 0;
for (const [license, pkgs] of Object.entries(byLicense)) {
  for (const pkg of pkgs) {
    count++;
    if (!isAllowed(license)) rejected.push(`${pkg.name}@${pkg.versions.join(",")}: ${license}`);
  }
}

if (rejected.length > 0) {
  console.error(`Disallowed licenses in shipped JS dependencies:\n  ${rejected.join("\n  ")}`);
  process.exit(1);
}
console.log(`licenses ok (${count} production packages)`);
