// The outline rail (M2 task 9): the document's headings on the left, the
// current section highlighted, a click goes there. The list is virtualized
// (a 20 MB document has tens of thousands of headings): only the rows on
// screen are in the DOM. The headings come from the core once the index is
// complete (`md_outline`), again whenever the tables change.

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

const ROW_H = 24;
const OVERSCAN = 8;

/** A heading: start byte of its block, level, text. */
export type OutlineEntry = [number, number, string];

interface OutlineReply {
  generation: number;
  entries: OutlineEntry[];
}

/** The last entry starting at or before `offset` (-1: none). */
export function currentIndex(entries: readonly OutlineEntry[], offset: number): number {
  let lo = 0;
  let hi = entries.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (entries[mid]![0] <= offset) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

export class OutlineRail {
  readonly element: HTMLElement;
  private readonly list: HTMLDivElement;
  private readonly spacer: HTMLDivElement;
  private readonly empty: HTMLDivElement;
  private entries: OutlineEntry[] = [];
  private generation: number | undefined;
  private current = -1;
  private frame = 0;
  private fetching = false;
  private again = false;
  private destroyed = false;
  private minLevel = 1;

  constructor(
    private readonly doc: number,
    /** A heading was chosen: its block's start byte. */
    private readonly go: (offset: number) => void,
  ) {
    this.element = document.createElement("nav");
    this.element.className = "md-outline";
    this.element.setAttribute("aria-label", t("md.outline"));
    this.list = document.createElement("div");
    this.list.className = "md-outline-list";
    this.list.tabIndex = 0;
    this.list.setAttribute("role", "tree");
    this.spacer = document.createElement("div");
    this.spacer.className = "md-outline-spacer";
    this.list.append(this.spacer);
    this.empty = document.createElement("div");
    this.empty.className = "md-outline-empty";
    this.empty.textContent = t("md.outlineIndexing");
    this.element.append(this.empty, this.list);
    this.list.addEventListener("scroll", () => {
      this.schedule();
    });
    this.list.addEventListener("click", (e) => {
      const row = (e.target as HTMLElement).closest<HTMLElement>(".md-outline-row");
      const i = Number(row?.dataset.index);
      const entry = this.entries[i];
      if (entry) this.go(entry[0]);
    });
  }

  /** Asks the core for the headings if they changed. */
  refresh(): void {
    if (this.destroyed) return;
    if (this.fetching) {
      this.again = true;
      return;
    }
    this.fetching = true;
    void invoke<OutlineReply | null>("md_outline", { doc: this.doc, since: this.generation })
      .then((r) => {
        if (r && !this.destroyed) {
          this.generation = r.generation;
          this.entries = r.entries;
          this.minLevel = r.entries.reduce((m, e) => Math.min(m, e[1]), 6);
          this.empty.textContent = t("md.outlineEmpty");
          this.empty.hidden = r.entries.length > 0;
          this.current = -1;
          this.schedule();
        }
      })
      .catch(() => {
        // The next change asks again.
      })
      .finally(() => {
        this.fetching = false;
        if (this.again) {
          this.again = false;
          this.refresh();
        }
      });
  }

  /** The visible pane's top moved to `offset`: highlight its section. */
  setPosition(offset: number): void {
    const i = currentIndex(this.entries, offset);
    if (i === this.current) return;
    this.current = i;
    // Keep the current heading in view.
    const y = i * ROW_H;
    const top = this.list.scrollTop;
    const h = this.list.clientHeight;
    if (i >= 0 && h > 0 && (y < top || y + ROW_H > top + h)) {
      this.list.scrollTop = Math.max(0, y - h / 3);
    }
    this.schedule();
  }

  /** Entries (tests, benchmarks). */
  headings(): readonly OutlineEntry[] {
    return this.entries;
  }

  private schedule(): void {
    if (this.frame !== 0 || this.destroyed) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  private draw(): void {
    this.spacer.style.height = `${this.entries.length * ROW_H}px`;
    const h = this.list.clientHeight;
    if (h === 0) return;
    const first = Math.max(0, Math.floor(this.list.scrollTop / ROW_H) - OVERSCAN);
    const last = Math.min(
      this.entries.length,
      Math.ceil((this.list.scrollTop + h) / ROW_H) + OVERSCAN,
    );
    const rows: HTMLElement[] = [];
    for (let i = first; i < last; i++) {
      const [, level, text] = this.entries[i]!;
      const row = document.createElement("div");
      row.className = i === this.current ? "md-outline-row current" : "md-outline-row";
      row.dataset.index = String(i);
      row.setAttribute("role", "treeitem");
      row.setAttribute("aria-level", String(level));
      if (i === this.current) row.setAttribute("aria-current", "location");
      row.style.top = `${i * ROW_H}px`;
      row.style.paddingLeft = `${8 + (level - this.minLevel) * 12}px`;
      row.textContent = text || t("md.outlineUntitled");
      row.title = text;
      rows.push(row);
    }
    this.spacer.replaceChildren(...rows);
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.frame);
    this.element.remove();
  }
}
