// Performance HUD (§7; M0 task 10): a developer overlay toggled with
// Ctrl+Shift+H. Plain DOM, outside React, and idle while hidden: the frame
// sampler and the stats poll only run while it is open.

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";
import { openSurfaces } from "../surface/surface";
import { summarize } from "./stats";
import { getLongTaskCount } from "./trace";

interface JobRecord {
  name: string;
  priority: string;
  queued_us: number;
  run_us: number;
  outcome: string;
}

interface PerfStats {
  scheduler: {
    queued: number[];
    running: number;
    completed: number;
    cancelled: number;
    stale_dropped: number;
    coalesced: number;
    panicked: number;
  };
  recentJobs: JobRecord[];
  memory: {
    total_bytes: number;
    global_budget: number;
    caches: { name: string; class: string; bytes: number; evicted_bytes: number }[];
  };
}

const FRAME_HISTORY = 240;
const POLL_MS = 500;
const PRIORITIES = ["Interaction", "Visible", "Near", "Prefetch", "Background"];

const mib = (b: number) => `${(b / 1048576).toFixed(1)} MiB`;

/** Escapes text for innerHTML; job names and transports come from the core. */
function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

class Hud {
  private readonly root: HTMLDivElement;
  private readonly graph: HTMLCanvasElement;
  private readonly text: HTMLPreElement;
  private readonly frames: number[] = [];
  private last = 0;
  private rafHandle = 0;
  private pollHandle = 0;
  private stats: PerfStats | undefined;

  constructor() {
    this.root = document.createElement("div");
    this.root.className = "perf-hud";
    this.root.setAttribute("role", "status");
    this.root.setAttribute("aria-label", t("hud.title"));
    this.graph = document.createElement("canvas");
    this.graph.width = FRAME_HISTORY;
    this.graph.height = 48;
    this.text = document.createElement("pre");
    this.root.append(this.graph, this.text);
  }

  get open(): boolean {
    return this.root.isConnected;
  }

  show(): void {
    document.body.append(this.root);
    this.last = performance.now();
    const tick = (now: number) => {
      this.frames.push(now - this.last);
      if (this.frames.length > FRAME_HISTORY) this.frames.shift();
      this.last = now;
      this.drawGraph();
      this.rafHandle = requestAnimationFrame(tick);
    };
    this.rafHandle = requestAnimationFrame(tick);
    const poll = () => {
      invoke<PerfStats>("perf_stats")
        .then((s) => {
          this.stats = s;
          this.render();
        })
        .catch((err: unknown) => {
          console.error("perf_stats failed", err);
        });
    };
    poll();
    this.pollHandle = window.setInterval(poll, POLL_MS);
  }

  hide(): void {
    cancelAnimationFrame(this.rafHandle);
    clearInterval(this.pollHandle);
    this.frames.length = 0;
    this.root.remove();
  }

  private drawGraph(): void {
    const ctx = this.graph.getContext("2d");
    if (!ctx) return;
    const { width, height } = this.graph;
    ctx.clearRect(0, 0, width, height);
    // Reference lines at 16.7 ms (60 fps) and 50 ms (§4 hard limit).
    const scale = height / 60;
    ctx.fillStyle = "rgb(255 255 255 / 25%)";
    ctx.fillRect(0, height - 16.7 * scale, width, 1);
    ctx.fillStyle = "rgb(255 80 80 / 50%)";
    ctx.fillRect(0, height - 50 * scale, width, 1);
    this.frames.forEach((ms, i) => {
      ctx.fillStyle = ms > 50 ? "#ff5050" : ms > 17 ? "#ffb020" : "#5ad17a";
      const h = Math.min(height, ms * scale);
      ctx.fillRect(i, height - h, 1, h);
    });
  }

  private render(): void {
    const f = summarize(this.frames);
    const lines = [
      `<b>${esc(t("hud.title"))}</b>  (Ctrl+Shift+H)`,
      `${esc(t("hud.frames"))}  p50 ${f.p50} ms  p95 ${f.p95} ms  max ${f.max} ms`,
      `${esc(t("hud.longTasks"))}  ${getLongTaskCount()}`,
    ];
    const s = this.stats;
    if (s) {
      const q = s.scheduler.queued.map((n, i) => `${PRIORITIES[i] ?? i} ${n}`).join("  ");
      lines.push(
        `${esc(t("hud.queues"))}  ${q}  running ${s.scheduler.running}`,
        `${esc(t("hud.jobs"))}  done ${s.scheduler.completed}  stale ${s.scheduler.stale_dropped}  cancelled ${s.scheduler.cancelled}  coalesced ${s.scheduler.coalesced}  panicked ${s.scheduler.panicked}`,
        `${esc(t("hud.memory"))}  ${mib(s.memory.total_bytes)} / ${mib(s.memory.global_budget)}`,
      );
      for (const c of s.memory.caches) {
        lines.push(
          `  ${esc(c.name)} (${c.class})  ${mib(c.bytes)}  evicted ${mib(c.evicted_bytes)}`,
        );
      }
    }
    for (const surface of openSurfaces()) {
      const d = surface.diagnostics();
      if (!d) continue;
      lines.push(
        `${esc(t("hud.surface", { view: d.view }))}  ${esc(d.transport)}  tiles ${d.cachedTiles}/${d.cacheCapacity}  received ${d.counters.received}  dropped ${d.counters.droppedUnwanted}  requests ${d.counters.requests}`,
      );
    }
    if (s && s.recentJobs.length > 0) {
      lines.push(esc(t("hud.recentJobs")));
      for (const j of s.recentJobs.slice(-10).reverse()) {
        lines.push(
          `  ${esc(j.name).padEnd(14)} ${j.priority.padEnd(12)} wait ${(j.queued_us / 1000).toFixed(2)} ms  run ${(j.run_us / 1000).toFixed(2)} ms  ${j.outcome}`,
        );
      }
    }
    this.text.innerHTML = lines.join("\n");
  }
}

let hud: Hud | undefined;

/** Installs the Ctrl+Shift+H toggle. */
export function installHud(): void {
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "h") {
      e.preventDefault();
      hud ??= new Hud();
      if (hud.open) hud.hide();
      else hud.show();
    }
  });
}
