import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";
import { notify, useDocState } from "./txt/doc-store";
import { EditorScreen, Notices } from "./txt/EditorScreen";
import { Session, getUi, setUi } from "./txt/session";

interface Orphan {
  journal: string;
  header: { path: string | null };
  records: number;
  lastChangeMs: number | null;
}

/** Progress of a non-UTF-8 file being decoded before it opens, with Cancel. */
function Decoding() {
  const { opening } = useDocState();
  if (!opening) return null;
  const pct = opening.total > 0 ? Math.floor((opening.done / opening.total) * 100) : 0;
  return (
    <div className="banner" role="status">
      <span>{t("app.decoding", { pct })}</span>
      <progress max={1} value={opening.total > 0 ? opening.done / opening.total : 0} />
      <button
        type="button"
        onClick={() => {
          void invoke("txt_open_cancel");
        }}
      >
        {t("common.cancel")}
      </button>
    </div>
  );
}

/** The session the app shows; exposed for benchmarks and UI tests. */
export const appState: { session: Session | undefined } = { session: undefined };
// UI tests (WebDriver) read it to check positions; never exposed otherwise.
if (navigator.webdriver) {
  (window as unknown as { __quire?: typeof appState }).__quire = appState;
}

export function App() {
  const [session, setSession] = useState<Session | undefined>();
  const [orphans, setOrphans] = useState<Orphan[]>([]);
  const [wrap, setWrap] = useState(false);
  const sessionRef = useRef<Session | undefined>(undefined);

  const show = useCallback((s: Session | undefined) => {
    sessionRef.current = s;
    appState.session = s;
    setSession(s);
  }, []);

  /** Replaces the current document (asking about unsaved edits first). */
  const replace = useCallback(
    async (next: () => Promise<Session | undefined>) => {
      const cur = sessionRef.current;
      if (cur && !(await cur.close())) return;
      show(undefined);
      try {
        const s = await next();
        if (s) show(s);
      } catch (err) {
        notify("error", String(err), 0);
      }
    },
    [show],
  );

  const openFile = useCallback(
    async (path?: string) => {
      const p = path ?? (await openDialog({ multiple: false, directory: false }));
      if (!p) return;
      await replace(() => Session.open(p));
    },
    [replace],
  );

  // Startup: a file from the command line (already opening in the core),
  // and journals left by a crash.
  useEffect(() => {
    Session.startup()
      .then((s) => {
        if (s) show(s);
      })
      .catch((err: unknown) => {
        // The welcome screen stays; say why the file isn't open.
        notify("error", t("app.openFailed", { error: String(err) }), 0);
      })
      .finally(() => {
        performance.measure("txt-startup-ready");
      });
    void invoke<Orphan[]>("txt_recovery_list").then(setOrphans);
  }, [show]);

  // Commands from the editor's keyboard handler.
  useEffect(() => {
    if (!session) return;
    session.setCommandHandler((name) => {
      switch (name) {
        case "find":
          setUi({ find: getUi().find === "replace" ? "replace" : "find" });
          break;
        case "replace":
          setUi({ find: "replace" });
          break;
        case "findNext":
        case "findPrevious":
          if (!session.findRepeat(name === "findPrevious")) setUi({ find: "find" });
          break;
        case "gotoLine":
          setUi({ goto: true });
          break;
        case "save":
          void session.save(false);
          break;
        case "saveAs":
          void session.save(true);
          break;
        case "open":
          void openFile();
          break;
        case "new":
          void replace(() => Session.create_new());
          break;
        case "toggleWrap":
          setWrap((w) => !w);
          break;
      }
    });
  }, [session, openFile, replace]);

  // Shortcuts that work without a document.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (sessionRef.current || !(e.ctrlKey || e.metaKey)) return;
      if (e.key.toLowerCase() === "o") {
        e.preventDefault();
        void openFile();
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        void replace(() => Session.create_new());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [openFile, replace]);

  // Drag and drop a file onto the window.
  useEffect(() => {
    const un = listen<{ paths: string[] }>("tauri://drag-drop", (e) => {
      const p = e.payload.paths[0];
      if (p) void openFile(p);
    });
    return () => {
      void un.then((f) => {
        f();
      });
    };
  }, [openFile]);

  // Closing the window with unsaved edits asks first.
  useEffect(() => {
    const w = getCurrentWindow();
    const un = w.onCloseRequested(async (e) => {
      const s = sessionRef.current;
      if (!s) return;
      e.preventDefault();
      if (await s.close()) await w.destroy();
    });
    return () => {
      void un.then((f) => {
        f();
      });
    };
  }, []);

  if (session) {
    return (
      <EditorScreen
        key={session.doc}
        session={session}
        wrap={wrap}
        onToggleWrap={() => {
          setWrap((w) => !w);
        }}
      />
    );
  }
  return (
    <main className="empty">
      <Decoding />
      <div className="welcome">
        <p>{t("app.empty")}</p>
        <div className="welcome-actions">
          <button
            type="button"
            onClick={() => {
              void openFile();
            }}
          >
            {t("app.open")}
          </button>
          <button
            type="button"
            onClick={() => {
              void replace(() => Session.create_new());
            }}
          >
            {t("app.new")}
          </button>
        </div>
        {orphans.length > 0 && (
          <div className="recovery" role="region" aria-label={t("recovery.title")}>
            <h2>{t("recovery.title")}</h2>
            {orphans.map((o) => (
              <div className="recovery-item" key={o.journal}>
                <span>
                  {o.header.path ?? t("recovery.untitled")} —{" "}
                  {t("recovery.changes", { count: o.records })}
                  {o.lastChangeMs ? ` · ${new Date(o.lastChangeMs).toLocaleString()}` : ""}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    setOrphans(orphans.filter((x) => x !== o));
                    void replace(() => Session.recover(o.journal));
                  }}
                >
                  {t("recovery.restore")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setOrphans(orphans.filter((x) => x !== o));
                    void invoke("txt_recovery_discard", { journalPath: o.journal });
                  }}
                >
                  {t("recovery.discard")}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
      <Notices />
    </main>
  );
}
