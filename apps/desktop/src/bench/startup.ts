// Startup scenario (§4: cold start ≤ 1.0 s, warm ≤ 0.4 s to interactive).
// "Interactive" = the app's first frame is painted; the core adds the OS
// process creation time to the report, so the harness gets
// process creation → first frame.

export function run(): Promise<unknown> {
  const firstFrame = performance.getEntriesByName("first-frame", "measure")[0];
  if (!firstFrame) throw new Error("first-frame measure missing");
  const nav = performance.getEntriesByType("navigation")[0] as
    PerformanceNavigationTiming | undefined;
  return Promise.resolve({
    scenario: "startup",
    timeOriginEpochMs: performance.timeOrigin,
    firstFrameEpochMs: performance.timeOrigin + firstFrame.duration,
    navigation: nav && {
      responseEnd: nav.responseEnd,
      domContentLoaded: nav.domContentLoadedEventEnd,
      loadEventEnd: nav.loadEventEnd,
    },
  });
}
