// One open TXT document in the frontend: the core session id, its event
// channel, the editor surface, and the commands the chrome triggers.

import { Channel, invoke } from "@tauri-apps/api/core";
import { ask, message, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { t } from "../i18n";
import { getState, notify, reset, update, type DiskStatus, type DocStatus } from "./doc-store";
import { MdController } from "../md/controller";
import { TxtView } from "./txt-view";

export interface Query {
  pattern: string;
  regex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
}

interface DocInfo {
  doc: number;
  status: DocStatus;
  openMs: number;
}

type CoreEvent =
  | ({ kind: "status" } & DocStatus)
  | {
      kind: "search";
      generation: number;
      revision: number;
      progress: {
        bytesDone: number;
        bytesTotal: number;
        matches: number;
        truncated: boolean;
        complete: boolean;
      };
      firstHitMs: number | null;
      ms: number;
    }
  | { kind: "searchError"; error: string }
  | { kind: "replaceProgress"; fraction: number }
  | {
      kind: "replaced";
      count?: number;
      cancelled?: boolean;
      error?: string;
      ms?: number;
      materialized?: boolean;
    }
  | { kind: "disk"; status: DiskStatus }
  | { kind: "reloaded"; change: string }
  | { kind: "sharedWriter" }
  | { kind: "saving" }
  | { kind: "opening"; done: number; total: number }
  | { kind: "opened" };

export type UiMode = { find: "off" | "find" | "replace"; goto: boolean };

let ui: UiMode = { find: "off", goto: false };
const uiListeners = new Set<() => void>();

export function getUi(): UiMode {
  return ui;
}

export function setUi(patch: Partial<UiMode>): void {
  ui = { ...ui, ...patch };
  for (const l of uiListeners) l();
}

export function subscribeUi(l: () => void): () => void {
  uiListeners.add(l);
  return () => uiListeners.delete(l);
}

export class Session {
  view: TxtView | undefined;
  /** Markdown panes, for a Markdown document (ADR-0030). */
  md: MdController | undefined;
  private lastSearchRefresh = 0;
  private query: Query | undefined;
  private searchTimer = 0;
  private commandHandler: (name: string) => void = () => {};
  private repeatHandler: ((backwards: boolean) => void) | undefined;

  /** Keyboard commands from the editor go to `f` (the app chrome). */
  setCommandHandler(f: (name: string) => void): void {
    this.commandHandler = f;
  }

  /** The open find panel repeats its search (F3) through `f`. */
  setFindRepeat(f: ((backwards: boolean) => void) | undefined): void {
    this.repeatHandler = f;
  }

  /** Repeats the find panel's search; false if no panel is open. */
  findRepeat(backwards: boolean): boolean {
    if (!this.repeatHandler) return false;
    this.repeatHandler(backwards);
    return true;
  }

  private constructor(readonly doc: number) {}

  static channel(onEvent: (e: CoreEvent) => void): Channel<CoreEvent> {
    const c = new Channel<CoreEvent>();
    c.onmessage = onEvent;
    return c;
  }

  private static async create(
    open: (channel: Channel<CoreEvent>) => Promise<DocInfo | null>,
  ): Promise<Session | undefined> {
    const holder: { session?: Session } = {};
    const early: CoreEvent[] = [];
    let opened = false;
    const channel = Session.channel((e) => {
      // Decode progress while opening; later ones (reopening with another
      // encoding) go to the session.
      if (e.kind === "opening" && !opened) update({ opening: { done: e.done, total: e.total } });
      else if (holder.session) holder.session.onEvent(e);
      else early.push(e);
    });
    let info: DocInfo | null;
    try {
      info = await open(channel);
    } catch (err) {
      // A decode the user cancelled is not an error.
      if (String(err).includes("cancelled")) return undefined;
      throw err;
    } finally {
      opened = true;
      update({ opening: undefined });
    }
    if (!info) return undefined;
    reset();
    const session = new Session(info.doc);
    holder.session = session;
    update({ status: info.status });
    performance.measure("txt-open", {
      start: performance.now() - info.openMs,
      duration: info.openMs,
    });
    for (const e of early) session.onEvent(e);
    return session;
  }

  /** The document the app was launched with, if any (opened before the webview). */
  static startup(): Promise<Session | undefined> {
    return Session.create((channel) => invoke<DocInfo | null>("txt_startup", { channel }));
  }

  static open(path: string, encoding?: string): Promise<Session | undefined> {
    return Session.create((channel) =>
      invoke<DocInfo>("txt_open", { path, encoding: encoding ?? null, channel }),
    );
  }

  static create_new(): Promise<Session | undefined> {
    return Session.create((channel) => invoke<DocInfo>("txt_new", { channel }));
  }

  static recover(journalPath: string): Promise<Session | undefined> {
    return Session.create((channel) => invoke<DocInfo>("txt_recover", { journalPath, channel }));
  }

  /** Builds the editor in `host`; a Markdown document also gets its preview
   *  in `previewHost` and the view modes on `root`. */
  attach(host: HTMLElement, md?: { root: HTMLElement; previewHost: HTMLElement }): TxtView {
    this.view = new TxtView(host, this.doc, {
      onHeader: (header) => {
        this.md?.textChanged(header.revision);
        const st = getState().status;
        update({
          header,
          status: st ? { ...st, dirty: header.dirty, readOnly: header.readOnly } : st,
        });
        // Matches are stale after an edit: search again (debounced).
        if (this.query && getState().search && st && header.revision !== st.revision) {
          this.scheduleSearch();
        }
      },
      onCaret: (caret) => {
        update({ caret });
      },
      onError: (msg) => {
        if (msg.includes("busy")) notify("info", t("txt.busy"));
        else if (msg.includes("read-only")) notify("info", t("txt.readOnlyEdit"));
        else notify("error", msg);
      },
      onCommand: (name) => {
        this.command(name);
      },
      onScroll: (offset) => {
        this.md?.sourceScrolled(offset);
      },
    });
    this.view.focus();
    if (md && getState().status?.markdown) {
      const view = this.view;
      MdController.attach(this.doc, md.root, view, md.previewHost, (name) => {
        this.command(name);
      })
        .then((c) => {
          if (this.view !== view) c.destroy();
          else this.md = c;
        })
        .catch((err: unknown) => {
          notify("error", String(err));
        });
    }
    return this.view;
  }

  /** A keyboard command: view modes go to the Markdown panes, the rest to
   *  the chrome. */
  command(name: string): void {
    if (name === "toggleView" && this.md) this.md.toggleView();
    else if (name === "toggleSplit" && this.md) this.md.toggleSplit();
    else if (name === "toggleOutline" && this.md) this.md.toggleOutline();
    else this.commandHandler(name);
  }

  private onEvent(e: CoreEvent): void {
    switch (e.kind) {
      case "status": {
        const status: DocStatus = e;
        const prev = getState().status;
        update({ status, saving: status.busy && getState().saving });
        if (prev && prev.lineCount.exact !== status.lineCount.exact) this.view?.refresh();
        if (prev && !prev.index.complete && status.index.complete) this.view?.refresh();
        break;
      }
      case "search": {
        const p = e.progress;
        update({
          search: {
            generation: e.generation,
            matches: p.matches,
            complete: p.complete,
            truncated: p.truncated,
            firstHitMs: e.firstHitMs,
            ms: e.ms,
            bytesDone: p.bytesDone,
            bytesTotal: p.bytesTotal,
          },
        });
        const now = performance.now();
        if (p.complete || now - this.lastSearchRefresh > 250) {
          this.lastSearchRefresh = now;
          this.view?.refresh();
        }
        break;
      }
      case "searchError":
        notify("error", e.error);
        break;
      case "replaceProgress":
        update({ replaceProgress: e.fraction });
        break;
      case "replaced":
        update({ replaceProgress: undefined });
        if (e.error) notify("error", e.error);
        else if (e.cancelled) notify("info", t("find.replaceCancelled"));
        else notify("info", t("find.replaced", { count: e.count ?? 0 }));
        this.view?.refresh();
        if (this.query) this.scheduleSearch();
        break;
      case "disk":
        update({ disk: e.status });
        break;
      case "reloaded":
        notify("info", t("disk.reloaded"));
        update({ disk: undefined });
        this.view?.refresh();
        break;
      case "sharedWriter":
        update({ sharedWriter: true });
        break;
      case "saving":
        update({ saving: true });
        break;
      // Reopening with another encoding (decode progress, then done).
      case "opening":
        update({ opening: { done: e.done, total: e.total } });
        break;
      case "opened":
        update({ opening: undefined });
        break;
    }
  }

  // ---- saving ----------------------------------------------------------------

  /** Saves; for an untitled document or `as`, asks for a path first. */
  async save(as = false): Promise<boolean> {
    let path: string | null = null;
    const current = getState().status?.path ?? null;
    if (as || current === null) {
      path = await saveDialog({ defaultPath: current ?? "untitled.txt" });
      if (path === null) return false;
    }
    update({ saving: true });
    try {
      let out = await invoke<SaveOutcome>("txt_save", {
        doc: this.doc,
        path,
        replaceUnmappable: false,
      });
      if (out.result === "needsConsent") {
        const go = await ask(t("save.unmappable", { count: out.count, sample: out.sample }), {
          title: t("save.lossTitle"),
          kind: "warning",
        });
        if (!go) return false;
        out = await invoke<SaveOutcome>("txt_save", {
          doc: this.doc,
          path,
          replaceUnmappable: true,
        });
      }
      if (out.result === "saved") {
        const r = out.report;
        if (r.losses.length > 0) {
          await message(r.losses.join("\n"), { title: t("save.lossTitle"), kind: "warning" });
        }
        if (r.historyTrimmed > 0)
          notify("info", t("save.historyTrimmed", { count: r.historyTrimmed }));
        performance.measure("txt-save", {
          start: performance.now() - r.millis,
          duration: r.millis,
          detail: r,
        });
        update({ disk: undefined });
        this.view?.refresh();
        return true;
      }
      return false;
    } catch (err) {
      notify("error", t("save.failed", { error: String(err) }), 0);
      return false;
    } finally {
      update({ saving: false });
    }
  }

  async setFormat(encoding: string, bom: boolean, lineEnding: string | null): Promise<void> {
    const status = await invoke<DocStatus>("txt_set_format", {
      doc: this.doc,
      encoding,
      bom,
      lineEnding,
    });
    update({ status });
  }

  async reopen(encoding: string): Promise<void> {
    try {
      const status = await invoke<DocStatus>("txt_reopen", { doc: this.doc, encoding });
      update({ status });
      this.view?.refresh();
    } catch (err) {
      if (String(err).includes("cancelled")) notify("info", t("app.reopenCancelled"));
      else notify("error", String(err));
    } finally {
      update({ opening: undefined });
    }
  }

  async allowEdit(): Promise<void> {
    const status = await invoke<DocStatus>("txt_allow_edit", { doc: this.doc });
    update({ status });
    this.view?.refresh();
  }

  // ---- search ------------------------------------------------------------------

  private scheduleSearch(): void {
    clearTimeout(this.searchTimer);
    this.searchTimer = window.setTimeout(() => {
      if (this.query) void this.search(this.query);
    }, 300);
  }

  async search(q: Query | undefined): Promise<void> {
    this.query = q?.pattern ? q : undefined;
    if (!this.query) {
      await invoke("txt_search_cancel", { doc: this.doc });
      update({ search: undefined });
      this.view?.refresh();
      return;
    }
    const ignore = new Channel<ArrayBuffer>();
    try {
      await invoke("txt_search", { doc: this.doc, query: this.query, channel: ignore });
    } catch (err) {
      update({ search: undefined });
      notify("error", String(err));
    }
  }

  async find(q: Query, backwards: boolean, replace?: string): Promise<void> {
    try {
      const hit = await invoke<[number, number] | null>("txt_find", {
        doc: this.doc,
        query: q,
        backwards,
        replace: replace ?? null,
      });
      if (hit) await this.view?.reveal(hit[0]);
      else notify("info", t("find.none"));
    } catch (err) {
      notify("error", String(err));
    }
  }

  async replaceAll(q: Query, replacement: string): Promise<void> {
    try {
      update({ replaceProgress: 0 });
      await invoke("txt_replace_all", { doc: this.doc, query: q, replacement });
    } catch (err) {
      update({ replaceProgress: undefined });
      notify("error", String(err));
    }
  }

  async cancel(): Promise<void> {
    await invoke("txt_cancel", { doc: this.doc });
  }

  /** Jumps to 1-based `line`; `ms` is the core's lookup, the others the steps. */
  async gotoLine(
    line: number,
  ): Promise<{ exact: boolean; ms: number; invokeMs: number; revealMs: number }> {
    const t0 = performance.now();
    const r = await invoke<{ offset: number; exact: boolean; ms: number }>("txt_goto_line", {
      doc: this.doc,
      line: Math.max(0, line - 1),
    });
    const t1 = performance.now();
    await this.view?.reveal(r.offset);
    this.view?.focus();
    return { ...r, invokeMs: t1 - t0, revealMs: performance.now() - t1 };
  }

  // ---- disk --------------------------------------------------------------------

  async checkDisk(): Promise<void> {
    try {
      const s = await invoke<DiskStatus | null>("txt_check_disk", { doc: this.doc });
      if (s && s.change !== "touched") update({ disk: s });
    } catch {
      // A check that fails (e.g. a share that went away) shows on the next try.
    }
  }

  async resolveDisk(choice: "reload" | "keep" | "compare"): Promise<unknown> {
    try {
      const r = await invoke<unknown>("txt_disk_resolve", { doc: this.doc, choice });
      if (choice !== "compare") {
        update({ disk: undefined });
        this.view?.refresh();
      }
      return r;
    } catch (err) {
      notify("error", String(err));
      return undefined;
    }
  }

  /** Closes without asking, dropping unsaved edits (benchmarks). */
  async discard(): Promise<void> {
    this.md?.destroy();
    this.md = undefined;
    await invoke("txt_close", { doc: this.doc, discard: true });
    this.view?.destroy();
    this.view = undefined;
    reset();
  }

  /** Closes; asks first if there are unsaved edits. Returns false if kept open. */
  async close(): Promise<boolean> {
    if (getState().status?.dirty) {
      const choice = await ask(t("close.unsaved"), { title: t("close.title"), kind: "warning" });
      if (!choice) return false;
    }
    this.md?.destroy();
    this.md = undefined;
    await invoke("txt_close", { doc: this.doc, discard: true });
    this.view?.destroy();
    this.view = undefined;
    reset();
    return true;
  }
}

type SaveOutcome =
  | {
      result: "saved";
      report: {
        losses: string[];
        historyTrimmed: number;
        millis: number;
        writeMillis: number;
        fsyncMillis: number;
        commitMillis: number;
        bytesWritten: number;
        rawCopied: number;
      };
    }
  | { result: "needsConsent"; count: number; sample: string };
