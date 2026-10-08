// Frontend side of tracing (M0 task 8).
//
// - Long tasks (> 50 ms on the UI thread) are always counted and logged: §2
//   treats any of them as a bug.
// - When the core records a trace (QUIRE_TRACE), performance measures and long
//   tasks are batched and merged into the same Chrome trace.

import { invoke } from "@tauri-apps/api/core";

interface ExternalEvent {
  name: string;
  category: string;
  startEpochMs: number;
  durationMs: number | null;
}

const FLUSH_INTERVAL_MS = 1000;

let longTaskCount = 0;
const longTaskListeners = new Set<(count: number, durationMs: number) => void>();

export function getLongTaskCount(): number {
  return longTaskCount;
}

export function onLongTask(listener: (count: number, durationMs: number) => void): () => void {
  longTaskListeners.add(listener);
  return () => longTaskListeners.delete(listener);
}

function toExternal(entry: PerformanceEntry): ExternalEvent {
  return {
    name: entry.entryType === "longtask" ? "longtask" : entry.name,
    category: `frontend.${entry.entryType}`,
    startEpochMs: performance.timeOrigin + entry.startTime,
    durationMs: entry.duration,
  };
}

export async function startTracing(): Promise<void> {
  const recording = await invoke<boolean>("trace_status").catch(() => false);
  const pending: ExternalEvent[] = [];

  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      if (entry.entryType === "longtask") {
        longTaskCount++;
        console.warn(`long task: ${entry.duration.toFixed(1)} ms`);
        for (const listener of longTaskListeners) listener(longTaskCount, entry.duration);
      }
      if (recording) pending.push(toExternal(entry));
    }
  });
  // `buffered` delivers entries recorded before this point (startup runs
  // while trace_status is still in flight).
  for (const type of ["longtask", "measure"]) {
    if (PerformanceObserver.supportedEntryTypes.includes(type)) {
      observer.observe({ type, buffered: true });
    }
  }

  if (!recording) return;
  setInterval(() => {
    if (pending.length === 0) return;
    const events = pending.splice(0);
    invoke("trace_ingest", { events }).catch((err: unknown) => {
      console.error("trace_ingest failed", err);
    });
  }, FLUSH_INTERVAL_MS);
}
