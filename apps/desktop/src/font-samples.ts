// The worker's font warm-up samples (src/fonts-worker.ts): the editor's and
// the preview's font stacks (styles.css) as canvas fonts, each with text in
// the scripts the first screen usually mixes.

const TXT = `14px "Cascadia Mono", Consolas, "D2Coding", "MS Gothic", ui-monospace, monospace`;
const BODY = `15px system-ui, "Segoe UI", "Malgun Gothic", sans-serif`;
const CODE = `13px "Cascadia Mono", Consolas, "D2Coding", ui-monospace, monospace`;
const TEXT = "Straße naïve 0123 {} 한국어 문서 🙂";

export const FONT_SAMPLES: [string, string][] = [
  [TXT, TEXT],
  [BODY, TEXT],
  [CODE, TEXT],
];
