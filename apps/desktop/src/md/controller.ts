// A Markdown document's panes (user decision C, ADR-0030): the source view
// (the TXT editor, ADR-0023) and the preview, shown one at a time (Ctrl+E
// switches, keeping the position) or side by side (Ctrl+\), the choice
// remembered. In split view the panes scroll together: the source map is
// the block index (block ↔ byte range), interpolated within blocks.

import { Channel, invoke } from "@tauri-apps/api/core";
import { notify, update } from "../txt/doc-store";
import type { TxtView } from "../txt/txt-view";
import { SourceDecorator } from "./decorate";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { t } from "../i18n";
import {
  ExportCancelled,
  PDF_MAX_BYTES,
  abortExport,
  buildExport,
  finishExport,
  pdfTooLarge,
  type Built,
  type ReportItem,
} from "./export";
import { OutlineRail } from "./outline";
import { KIND } from "./protocol";
import { MdPreview } from "./preview";

export type ViewMode = "source" | "preview" | "split";

const MODE_KEY = "quire.md.mode";
const OUTLINE_KEY = "quire.md.outline";
/** The outline is asked for again this long after the preview changed. */
const OUTLINE_REFRESH_MS = 400;

function savedOutline(): boolean {
  try {
    return localStorage.getItem(OUTLINE_KEY) === "on";
  } catch {
    return false;
  }
}

function savedMode(): ViewMode {
  try {
    const m = localStorage.getItem(MODE_KEY);
    if (m === "source" || m === "preview" || m === "split") return m;
  } catch {
    // Storage can be unavailable; the default stays.
  }
  return "source";
}

export interface MdEvent {
  kind: "indexed" | "words";
  ms?: number;
  blocks?: number;
  headings?: number;
  words?: number;
}

export interface MdStatus {
  indexed: boolean;
  blocks: number;
  words: number | undefined;
  remote: boolean;
  blocked: boolean;
  mode: ViewMode;
  /** The outline rail is shown. */
  outline: boolean;
  /** The caret is in a table (the table helpers apply). */
  inTable: boolean;
  /** An export in progress: building the page (with the fraction of the
   *  source done), waiting for a file name, or writing / printing it. */
  exporting:
    | { kind: "html" | "pdf"; phase: "building" | "choosing" | "saving"; fraction: number }
    | undefined;
}

export type TableActionName =
  | "format"
  | "insertRowBelow"
  | "deleteRow"
  | "insertColumnRight"
  | "deleteColumn"
  | "nextCell"
  | "prevCell";

let mdStatus: MdStatus | undefined;
const listeners = new Set<() => void>();

export function getMdStatus(): MdStatus | undefined {
  return mdStatus;
}

export function subscribeMd(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

function setMd(patch: Partial<MdStatus> | undefined): void {
  mdStatus = patch === undefined ? undefined : { ...(mdStatus ?? emptyStatus()), ...patch };
  for (const l of listeners) l();
}

function emptyStatus(): MdStatus {
  return {
    indexed: false,
    blocks: 0,
    words: undefined,
    remote: false,
    blocked: false,
    mode: savedMode(),
    outline: savedOutline(),
    inTable: false,
    exporting: undefined,
  };
}

export class MdController {
  readonly preview: MdPreview;
  private readonly decorator: SourceDecorator;
  private revision = 0;
  readonly outline: OutlineRail;
  private outlineOn: boolean;
  private outlineTimer = 0;
  mode: ViewMode;
  /** The pane the user last touched (wheel, keys, pointer) drives the
   *  other; a pane moved by sync never drives back (no feedback loop). */
  private driving: "source" | "preview" = "source";
  private destroyed = false;
  /** Cancels the export in progress. */
  private exportAbort: AbortController | undefined;

  private constructor(
    readonly doc: number,
    private readonly root: HTMLElement,
    private readonly source: TxtView,
    previewHost: HTMLElement,
    imagePrefix: string,
    private readonly frameUrl: string,
    command: (name: string) => void,
  ) {
    this.preview = new MdPreview(previewHost, doc, imagePrefix, frameUrl, {
      onScroll: (off) => {
        this.synced("preview", off);
        if (this.mode !== "source") this.outline.setPosition(off);
      },
      onSourceClick: (off) => {
        if (this.mode !== "split") return;
        this.source.send({ type: "click", offset: off, mode: "set", unit: "char" });
      },
      onBlocked: () => {
        if (!mdStatus?.blocked) setMd({ blocked: true });
      },
      onError: (msg) => {
        notify("error", msg);
      },
    });
    this.outline = new OutlineRail(doc, (off) => {
      this.goTo(off);
    });
    root.prepend(this.outline.element);
    this.outlineOn = savedOutline();
    root.dataset.outline = this.outlineOn ? "on" : "off";
    // Headings change with the text: ask again a moment after the preview
    // changed (only when the rail is shown; the core answers "unchanged"
    // cheaply).
    this.preview.onPatch(() => {
      if (!this.outlineOn) return;
      window.clearTimeout(this.outlineTimer);
      this.outlineTimer = window.setTimeout(() => {
        this.outline.refresh();
      }, OUTLINE_REFRESH_MS);
    });
    this.decorator = new SourceDecorator(doc, source, () => {
      this.caretMoved();
    });
    this.decorator.update(0);
    const touch = (pane: "source" | "preview") => () => {
      this.driving = pane;
    };
    for (const ev of ["wheel", "keydown", "pointerdown"] as const) {
      source.element.addEventListener(ev, touch("source"), { capture: true, passive: true });
      this.preview.element.addEventListener(ev, touch("preview"), { capture: true, passive: true });
    }
    this.mode = savedMode();
    this.apply();
    // Shortcuts while the preview has focus.
    this.preview.element.addEventListener("keydown", (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const k = e.key.toLowerCase();
      const names: Record<string, string> = {
        e: "toggleView",
        "\\": "toggleSplit",
        s: e.shiftKey ? "saveAs" : "save",
        o: e.shiftKey ? "toggleOutline" : "open",
        n: "new",
        f: "find",
      };
      const name = names[k];
      if (name) {
        e.preventDefault();
        command(name);
      }
    });
    if (this.mode === "preview") this.preview.focus();
  }

  /** Attaches Markdown state to the open TXT document and builds the preview. */
  static async attach(
    doc: number,
    root: HTMLElement,
    source: TxtView,
    previewHost: HTMLElement,
    command: (name: string) => void,
  ): Promise<MdController> {
    const events = new Channel<MdEvent>();
    setMd(emptyStatus());
    // The preview needs the image prefix before its first window; attach
    // first with a temporary channel holder.
    const pending: MdEvent[] = [];
    const box: { ctl?: MdController } = {};
    events.onmessage = (e) => {
      if (box.ctl) box.ctl.onEvent(e);
      else pending.push(e);
    };
    const holder = new MdPreviewChannel();
    const info = await invoke<{ imagePrefix: string; frameUrl: string }>("md_attach", {
      doc,
      channel: holder.channel,
      events,
    });
    const ctl = new MdController(
      doc,
      root,
      source,
      previewHost,
      info.imagePrefix,
      info.frameUrl,
      command,
    );
    box.ctl = ctl;
    holder.target = ctl.preview;
    for (const e of pending) ctl.onEvent(e);
    return ctl;
  }

  private onEvent(e: MdEvent): void {
    if (e.kind === "indexed") {
      setMd({ indexed: true, blocks: e.blocks ?? 0 });
      this.outline.refresh();
      performance.measure("md-index", {
        start: performance.now() - (e.ms ?? 0),
        duration: e.ms ?? 0,
      });
    } else {
      setMd({ words: e.words });
    }
  }

  /** Ctrl+E: editing ⇄ reading view, keeping the position (from split
   *  view, to the reading view). */
  toggleView(): void {
    this.setMode(this.mode === "preview" ? "source" : "preview");
  }

  /** Whether the caret is in a table, as far as the block map knows. */
  private caretInTable(): boolean {
    const p = this.source.primary();
    return p !== undefined && this.decorator.kindAt(p.head) === KIND.table;
  }

  /** The caret or the text changed: the table helpers' availability. */
  caretMoved(): void {
    const inTable = this.caretInTable();
    if (inTable !== (mdStatus?.inTable ?? false)) setMd({ inTable });
  }

  /** Tab / Shift+Tab: to the next or previous table cell when the caret
   *  is in a table. */
  onTab(shift: boolean): boolean {
    if (!this.caretInTable()) return false;
    this.tableAction(shift ? "prevCell" : "nextCell");
    return true;
  }

  /** A table helper (task 10): the core computes the replacement, applied
   *  as one edit if the text didn't change meanwhile. */
  tableAction(action: TableActionName): void {
    const p = this.source.primary();
    if (!p) return;
    void invoke<{ revision: number; from: number; to: number; text: string; caret: number } | null>(
      "md_table",
      { doc: this.doc, offset: p.head, action },
    )
      .then((r) => {
        if (!r) {
          // Not in a table after all: Tab inserts a tab.
          if (action === "nextCell" || action === "prevCell") this.source.send({ type: "tab" });
          return;
        }
        if (this.source.primary()?.revision !== r.revision) return;
        if (r.from !== r.to || r.text !== "") {
          this.source.send({ type: "replace", edits: [[r.from, r.to, r.text]] });
        }
        this.source.send({ type: "setSelections", ranges: [[r.caret, r.caret]] });
      })
      .catch((err: unknown) => {
        notify("error", String(err));
      });
  }

  /** Exports the document as HTML or PDF (task 11): builds the page with
   *  progress and Cancel shown, asks where, then says what wasn't carried
   *  over. */
  async exportAs(kind: "html" | "pdf"): Promise<void> {
    try {
      const report = await this.runExport(kind, async (built) => {
        if (kind === "pdf" && pdfTooLarge(built.bytes)) {
          const mb = (n: number) => (n / (1024 * 1024)).toFixed(0);
          notify("error", t("md.pdfTooLarge", { size: mb(built.bytes), limit: mb(PDF_MAX_BYTES) }));
          return null;
        }
        return saveDialog({
          defaultPath: `${built.title}.${kind}`,
          filters: [{ name: kind.toUpperCase(), extensions: [kind] }],
        });
      });
      if (report) notify("info", exportSummary(report.path, report.items));
    } catch (err) {
      if (err instanceof ExportCancelled) notify("info", t("md.exportCancelled"), 3000);
      else notify("error", t("md.exportFailed", { message: String(err) }));
    }
  }

  /** Exports to `path` without asking (UI flows, benchmarks); rejects with
   *  "cancelled" when the export is cancelled. */
  async exportTo(kind: "html" | "pdf", path: string): Promise<ReportItem[]> {
    const r = await this.runExport(kind, () => Promise.resolve(path));
    return r?.items ?? [];
  }

  /** Cancels the export in progress (the banner's Cancel). */
  cancelExport(): void {
    this.exportAbort?.abort();
  }

  /** Builds the page, then writes it where `choose` says (null: nowhere). */
  private async runExport(
    kind: "html" | "pdf",
    choose: (built: Built) => Promise<string | null>,
  ): Promise<{ path: string; items: ReportItem[] } | null> {
    if (this.exportAbort) throw new Error(t("md.exportBusy"));
    const abort = new AbortController();
    this.exportAbort = abort;
    let shown = -1;
    const show = (phase: "building" | "choosing" | "saving", fraction: number) => {
      // Re-render only when the shown percentage changes.
      const pct = Math.floor(fraction * 100);
      if (phase === mdStatus?.exporting?.phase && pct === shown) return;
      shown = pct;
      if (!this.destroyed) setMd({ exporting: { kind, phase, fraction } });
    };
    show("building", 0);
    let built: Built | undefined;
    try {
      built = await buildExport(
        this.doc,
        this.frameUrl,
        (f) => {
          show("building", f);
        },
        abort.signal,
      );
      show("choosing", 1);
      const path = await choose(built);
      if (abort.signal.aborted) throw new ExportCancelled();
      if (!path) {
        void abortExport(built.id);
        return null;
      }
      show("saving", 1);
      await finishExport(built, path, kind, abort.signal);
      return { path, items: built.report };
    } finally {
      this.exportAbort = undefined;
      if (!this.destroyed) setMd({ exporting: undefined });
    }
  }

  /** A pasted image (decision A): stored next to the document by the core,
   *  then its link is pasted (one undo step). */
  pasteImage(file: File): boolean {
    void file
      .arrayBuffer()
      .then((buf) =>
        invoke<string>("md_paste_image", buf, { headers: { "x-quire-doc": String(this.doc) } }),
      )
      .then((link) => {
        this.source.paste(`![](${link})`);
      })
      .catch((err: unknown) => {
        notify("error", t("md.pasteImageFailed", { message: String(err) }));
      });
    return true;
  }

  /** Ctrl+Shift+O: the outline rail on or off (remembered). */
  toggleOutline(): void {
    this.outlineOn = !this.outlineOn;
    this.root.dataset.outline = this.outlineOn ? "on" : "off";
    try {
      localStorage.setItem(OUTLINE_KEY, this.outlineOn ? "on" : "off");
    } catch {
      // Not remembered this time.
    }
    setMd({ outline: this.outlineOn });
    if (this.outlineOn) {
      this.outline.refresh();
      this.outline.setPosition(
        this.mode === "source" ? this.source.scrollOffset() : this.preview.topOffset(),
      );
    }
  }

  /** A heading in the outline was chosen: show it in the panes shown. */
  goTo(offset: number): void {
    if (this.mode !== "source") void this.preview.reveal(offset, 0);
    if (this.mode !== "preview") this.source.scrollToOffset(offset);
    this.outline.setPosition(offset);
  }

  /** Ctrl+\: split view on or off (off returns to the editing view). */
  toggleSplit(): void {
    this.setMode(this.mode === "split" ? "source" : "split");
  }

  setMode(m: ViewMode): void {
    if (m === this.mode) return;
    const from = this.mode;
    // The position: the top of the pane being left.
    const offset = from === "preview" ? this.preview.topOffset() : this.source.scrollOffset();
    this.mode = m;
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      // Not remembered this time.
    }
    this.apply();
    if (m === "preview" || m === "split") {
      if (from === "source") void this.preview.reveal(offset, 0);
    }
    if (m === "source" || m === "split") {
      if (from === "preview") this.source.scrollToOffset(offset);
    }
    if (m === "preview") this.preview.focus();
    else this.source.focus();
    performance.mark(`md-mode-${m}`);
  }

  private apply(): void {
    this.root.dataset.mode = this.mode;
    setMd({ mode: this.mode });
  }

  /** Called when the source view scrolled. */
  sourceScrolled(offset: number): void {
    this.decorator.update(this.revision);
    this.synced("source", offset);
    if (this.mode === "source") this.outline.setPosition(offset);
  }

  /** The text changed (a new view header from the core). */
  textChanged(revision: number): void {
    this.revision = revision;
    this.decorator.update(revision);
  }

  /** Scroll sync: only the pane the user is working in drives. */
  private synced(pane: "source" | "preview", offset: number): void {
    if (this.mode !== "split" || this.destroyed || pane !== this.driving) return;
    if (pane === "source") this.preview.scrollToOffset(offset);
    else this.source.scrollToOffset(offset);
  }

  async setRemote(on: boolean): Promise<void> {
    await invoke("md_set_remote", { doc: this.doc, on });
    setMd({ remote: on, blocked: on ? false : (mdStatus?.blocked ?? false) });
  }

  destroy(): void {
    this.exportAbort?.abort();
    this.destroyed = true;
    window.clearTimeout(this.outlineTimer);
    this.decorator.destroy();
    this.outline.destroy();
    this.preview.destroy();
    void invoke("md_detach", { doc: this.doc });
    setMd(undefined);
    update({});
  }
}

function exportSummary(path: string, report: readonly ReportItem[]): string {
  if (report.length === 0) return t("md.exported", { path });
  const items = report
    .slice(0, 5)
    .map((r) => `${r.source} (${r.reason})`)
    .join("; ");
  return t("md.exportedWithReport", { path, count: report.length, items });
}

/** Holds the push Channel until the preview exists (its windows go there). */
class MdPreviewChannel {
  readonly channel = new Channel<ArrayBuffer>();
  target: MdPreview | undefined;
  private early: ArrayBuffer[] = [];

  constructor() {
    this.channel.onmessage = (buf) => {
      if (this.target) {
        for (const b of this.early) this.target.receive(b);
        this.early = [];
        this.target.receive(buf);
      } else this.early.push(buf);
    };
  }
}
