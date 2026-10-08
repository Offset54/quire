// Debug helper: launch the app under WebDriver, wait, and run probes.
// QUIRE_APP=<exe> node tests/ui/debug.mjs <wait ms> [app args...]
// (with tauri-driver running)
import { WebDriver } from "./webdriver.mjs";

const wd = new WebDriver();
const [wait, ...args] = process.argv.slice(2);
await wd.start(process.env.QUIRE_APP, args);
await new Promise((r) => setTimeout(r, Number(wait ?? 4000)));
const probe = (expr) =>
  wd.execAsync(
    `const done = arguments[arguments.length - 1];
     Promise.resolve().then(() => ${expr}).then((v) => done('ok ' + JSON.stringify(v)), (e) => done('err ' + String(e)));`,
  );
console.log(await wd.exec("return document.body.innerText.slice(0, 1500)"));
console.log(
  await probe(
    "performance.getEntriesByType('measure').map((m) => m.name + ' ' + Math.round(m.duration))",
  ),
);
console.log(await probe("window.__TAURI_INTERNALS__.invoke('bench_env')"));
console.log(await probe("window.__TAURI_INTERNALS__.invoke('bench_memory')"));
await wd.stop();
