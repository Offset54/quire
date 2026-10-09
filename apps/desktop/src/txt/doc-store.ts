// State of the open TXT document shown by the chrome (status bar, find
// panel, banners). The editor surface itself is imperative (TxtView); this
// store only carries what React renders, updated from core events.

import { useSyncExternalStore } from "react";
import type { CaretInfo } from "./txt-view";
import type { ViewHeader } from "./protocol";

export interface EndingStats {
  lf: number;
  crlf: number;
  cr: number;
  nul: number;
}

export interface DocStatus {
  doc: number;
  path: string | null;
  len: number;
  lineCount: { line: number; exact: boolean };
  index: {
    bytes: number;
    knownBytes: number;
    complete: boolean;
    knownBreaks: number;
    endings: EndingStats;
  };
  indexMs?: number;
  encoding: string;
  encodingLabel: string;
  detectedBy: string;
  bom: boolean;
  eol: "lf" | "crlf" | "cr";
  convertEol: "lf" | "crlf" | "cr" | null;
  dirty: boolean;
  readOnly: boolean;
  binary: boolean;
  source: string;
  remote: string | null;
  revision: number;
  busy: boolean;
}

export interface DiskStatus {
  change: "touched" | "appended" | "modified" | "replaced" | "deleted";
  keepMinePossible: boolean;
}

export interface SearchState {
  generation: number;
  matches: number;
  complete: boolean;
  truncated: boolean;
  firstHitMs: number | null;
  ms: number;
  bytesDone: number;
  bytesTotal: number;
}

export interface Notice {
  id: number;
  kind: "info" | "warning" | "error";
  text: string;
}

export interface State {
  status: DocStatus | undefined;
  header: ViewHeader | undefined;
  caret: CaretInfo | undefined;
  disk: DiskStatus | undefined;
  search: SearchState | undefined;
  replaceProgress: number | undefined;
  saving: boolean;
  sharedWriter: boolean;
  notices: Notice[];
  /** A non-UTF-8 file being decoded before it opens (raw bytes). */
  opening: { done: number; total: number } | undefined;
}

let state: State = {
  status: undefined,
  header: undefined,
  caret: undefined,
  disk: undefined,
  search: undefined,
  replaceProgress: undefined,
  saving: false,
  sharedWriter: false,
  notices: [],
  opening: undefined,
};
const listeners = new Set<() => void>();
let noticeId = 0;

export function getState(): State {
  return state;
}

export function update(patch: Partial<State>): void {
  state = { ...state, ...patch };
  for (const l of listeners) l();
}

export function reset(): void {
  update({
    status: undefined,
    header: undefined,
    caret: undefined,
    disk: undefined,
    search: undefined,
    replaceProgress: undefined,
    saving: false,
    sharedWriter: false,
  });
}

export function notify(kind: Notice["kind"], text: string, ms = 6000): void {
  const id = ++noticeId;
  update({ notices: [...state.notices, { id, kind, text }] });
  if (ms > 0) {
    setTimeout(() => {
      dismiss(id);
    }, ms);
  }
}

export function dismiss(id: number): void {
  update({ notices: state.notices.filter((n) => n.id !== id) });
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useDocState(): State {
  return useSyncExternalStore(subscribe, getState);
}

/** Line-ending summary for the status bar: the kind, or "mixed". */
export function endingLabel(e: EndingStats | undefined): string {
  if (!e) return "";
  const kinds = [e.lf > 0 && "LF", e.crlf > 0 && "CRLF", e.cr > 0 && "CR"].filter(Boolean);
  if (kinds.length > 1) return "mixed";
  return (kinds[0] as string | undefined) ?? "";
}
