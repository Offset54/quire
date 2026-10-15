// Markdown parts of the editor chrome: the remote-content banner and the
// status bar items (view mode, word count, preview indexing).

import { useState, useSyncExternalStore } from "react";
import { t, type MessageId } from "../i18n";
import type { Session } from "../txt/session";
import { getMdStatus, subscribeMd, type TableActionName, type ViewMode } from "./controller";

const fmt = new Intl.NumberFormat();

export function useMdStatus() {
  return useSyncExternalStore(subscribeMd, getMdStatus);
}

/** Remote images are blocked (§8): say so, with the per-document toggle. */
export function MdBanner({ session }: { session: Session }) {
  const md = useMdStatus();
  if (!md?.blocked || md.remote) return null;
  return (
    <div className="banner" key="remote" role="status">
      <span>{t("md.remoteBlocked")}</span>
      <button
        type="button"
        onClick={() => {
          void session.md?.setRemote(true);
        }}
      >
        {t("md.loadRemote")}
      </button>
    </div>
  );
}

const TABLE_ACTIONS: [TableActionName, MessageId, string?][] = [
  ["format", "md.tableFormat", "Alt+Shift+F"],
  ["insertRowBelow", "md.tableInsertRow"],
  ["deleteRow", "md.tableDeleteRow"],
  ["insertColumnRight", "md.tableInsertColumn"],
  ["deleteColumn", "md.tableDeleteColumn"],
];

export function MdStatusItems({ session }: { session: Session }) {
  const md = useMdStatus();
  const [tableMenu, setTableMenu] = useState(false);
  if (!md) return null;
  const mode = md.mode;
  const button = (m: ViewMode, label: string, title: string) => (
    <button
      type="button"
      className={mode === m ? "on" : ""}
      title={title}
      onClick={() => {
        session.md?.setMode(m);
      }}
    >
      {label}
    </button>
  );
  return (
    <>
      {md.words !== undefined && <span>{t("md.words", { count: fmt.format(md.words) })}</span>}
      {md.inTable && (
        <button
          type="button"
          className={tableMenu ? "on" : ""}
          aria-haspopup="menu"
          onClick={() => {
            setTableMenu(!tableMenu);
          }}
        >
          {t("md.table")}
        </button>
      )}
      {md.inTable && tableMenu && (
        <div className="menu" role="menu">
          {TABLE_ACTIONS.map(([a, label, keys]) => (
            <button
              type="button"
              role="menuitem"
              key={a}
              title={keys}
              onClick={() => {
                setTableMenu(false);
                session.md?.tableAction(a);
                session.view?.focus();
              }}
            >
              {t(label)}
            </button>
          ))}
        </div>
      )}
      {!md.indexed && <span className="progress">{t("md.indexing")}</span>}
      {md.remote && (
        <button
          type="button"
          className="on"
          title={t("md.remoteOnTitle")}
          onClick={() => {
            void session.md?.setRemote(false);
          }}
        >
          {t("md.remoteOn")}
        </button>
      )}
      <button
        type="button"
        className={md.outline ? "on" : ""}
        title="Ctrl+Shift+O"
        onClick={() => {
          session.md?.toggleOutline();
        }}
      >
        {t("md.outline")}
      </button>
      <span className="segmented" role="group" aria-label={t("md.viewMode")}>
        {button("source", t("md.source"), "Ctrl+E")}
        {button("split", t("md.split"), "Ctrl+\\")}
        {button("preview", t("md.reading"), "Ctrl+E")}
      </span>
    </>
  );
}
