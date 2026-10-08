export interface Summary {
  n: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  mean: number;
}

export function summarize(values: number[]): Summary {
  const s = [...values].sort((a, b) => a - b);
  const pick = (p: number) =>
    s[Math.min(s.length - 1, Math.max(0, Math.ceil((p / 100) * s.length) - 1))] ?? NaN;
  const r = (v: number) => Math.round(v * 100) / 100;
  return {
    n: s.length,
    p50: r(pick(50)),
    p95: r(pick(95)),
    p99: r(pick(99)),
    max: r(s[s.length - 1] ?? NaN),
    mean: r(s.reduce((a, b) => a + b, 0) / Math.max(1, s.length)),
  };
}
