// Debug helper: launch the app under WebDriver and print the page.
// node tests/ui/debug.mjs <args...> (with tauri-driver running)
import { WebDriver } from "./webdriver.mjs";

const wd = new WebDriver();
await wd.start(process.env.QUIRE_APP, process.argv.slice(2));
await new Promise((r) => setTimeout(r, 4000));
console.log(await wd.exec("return document.body.innerText.slice(0, 2000)"));
console.log(
  await wd.execAsync(
    "const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke('txt_recovery_list').then((v) => done('ok ' + JSON.stringify(v)), (e) => done('err ' + String(e)));",
  ),
);
await wd.stop();
