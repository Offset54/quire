// A Markdown document's panes (user decision C, ADR-0030): the source view
// (the TXT editor, ADR-0023) and the preview, shown one at a time (Ctrl+E
// switches, keeping the position) or side by side (Ctrl+\), the choice
// remembered. In split view the panes scroll together: the source map is
// the block index (block ↔ byte range), interpolated within blocks.

import { Channel, invoke } from "@tauri-apps/api/core";
import { notify, update } from "../txt/doc-store";
import type { TxtView } from "../txt/txt-view";
import { SourceDecorator } from "./decorate";
import { MdPreview } from "./preview";

export type ViewMode = "source" | "preview" | "split";

const MODE_KEY = "quire.md.mode";

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
}

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
  };
}

export class MdController {
  readonly preview: MdPreview;
  private readonly decorator: SourceDecorator;
  private revision = 0;
  mode: ViewMode;
  /** The pane the user last touched (wheel, keys, pointer) drives the
   *  other; a pane moved by sync never drives back (no feedback loop). */
  private driving: "source" | "preview" = "source";
  private destroyed = false;

  private constructor(
    readonly doc: number,
    private readonly root: HTMLElement,
    private readonly source: TxtView,
    previewHost: HTMLElement,
    imagePrefix: string,
    command: (name: string) => void,
  ) {
    this.preview = new MdPreview(previewHost, doc, imagePrefix, {
      onScroll: (off) => {
        this.synced("preview", off);
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
    this.decorator = new SourceDecorator(doc, source);
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
        o: "open",
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
    const info = await invoke<{ imagePrefix: string }>("md_attach", {
      doc,
      channel: holder.channel,
      events,
    });
    const ctl = new MdController(doc, root, source, previewHost, info.imagePrefix, command);
    box.ctl = ctl;
    holder.target = ctl.preview;
    for (const e of pending) ctl.onEvent(e);
    return ctl;
  }

  private onEvent(e: MdEvent): void {
    if (e.kind === "indexed") {
      setMd({ indexed: true, blocks: e.blocks ?? 0 });
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
    this.destroyed = true;
    this.decorator.destroy();
    this.preview.destroy();
    void invoke("md_detach", { doc: this.doc });
    setMd(undefined);
    update({});
  }
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
