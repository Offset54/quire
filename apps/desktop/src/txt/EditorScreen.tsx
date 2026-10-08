// Chrome around the TXT editor surface: banners, find/replace, go to line,
// status bar. React renders only these; the text itself is TxtView's.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { t, type MessageId } from "../i18n";
import { dismiss, endingLabel, useDocState } from "./doc-store";
import { getUi, setUi, subscribeUi, type Query, type Session } from "./session";

const ENCODINGS = [
  "UTF-8",
  "UTF-16LE",
  "UTF-16BE",
  "EUC-KR",
  "Shift_JIS",
  "gb18030",
  "Big5",
  "windows-1250",
  "windows-1251",
  "windows-1252",
  "windows-1253",
  "windows-1254",
  "windows-1255",
  "windows-1256",
  "windows-1257",
  "windows-1258",
];

const fmt = new Intl.NumberFormat();

function useUi() {
  return useSyncExternalStore(subscribeUi, getUi);
}

function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  return `${(n / 1024 ** 3).toFixed(2)} GiB`;
}

export function EditorScreen({
  session,
  wrap,
  onToggleWrap,
}: {
  session: Session;
  wrap: boolean;
  onToggleWrap: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!host.current) return;
    const view = session.attach(host.current);
    view.setWrap(wrap);
    const onFocus = () => {
      void session.checkDisk();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
    };
    // The view lives as long as the session; wrap changes go through setWrap.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session]);
  useEffect(() => {
    session.view?.setWrap(wrap);
  }, [session, wrap]);
  const ui = useUi();
  return (
    <div className="editor">
      <Banners session={session} />
      {ui.find !== "off" && <FindPanel session={session} replace={ui.find === "replace"} />}
      {ui.goto && <GotoLine session={session} />}
      <div className="editor-surface" ref={host} />
      <StatusBar session={session} wrap={wrap} onToggleWrap={onToggleWrap} />
      <Notices />
    </div>
  );
}

function Notices() {
  const { notices } = useDocState();
  return (
    <div className="notices" role="status">
      {notices.map((n) => (
        <div key={n.id} className={`notice ${n.kind}`}>
          <span>{n.text}</span>
          <button
            type="button"
            aria-label={t("common.dismiss")}
            onClick={() => {
              dismiss(n.id);
            }}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

function Banners({ session }: { session: Session }) {
  const s = useDocState();
  const [hunks, setHunks] = useState<
    { mine: string; disk: string; mineLines: [number, number] }[] | null
  >(null);
  const banners: React.ReactNode[] = [];
  if (s.status?.binary && s.status.readOnly) {
    banners.push(
      <div className="banner warning" key="binary">
        <span>{t("banner.binary")}</span>
        <button
          type="button"
          onClick={() => {
            void session.allowEdit();
          }}
        >
          {t("banner.editAnyway")}
        </button>
      </div>,
    );
  }
  if (s.disk) {
    const d = s.disk;
    banners.push(
      <div className="banner warning" key="disk">
        <span>{t(`disk.${d.change}` as MessageId)}</span>
        <button
          type="button"
          onClick={() => {
            void session.resolveDisk("reload");
          }}
        >
          {t("disk.reload")}
        </button>
        <button
          type="button"
          disabled={!d.keepMinePossible}
          title={d.keepMinePossible ? "" : t("disk.keepImpossible")}
          onClick={() => {
            void session.resolveDisk("keep");
          }}
        >
          {t("disk.keep")}
        </button>
        <button
          type="button"
          onClick={() => {
            void session.resolveDisk("compare").then((r) => {
              const res = r as { hunks?: typeof hunks; tooLarge?: boolean } | undefined;
              setHunks(res?.hunks ?? []);
              if (res?.tooLarge) setHunks(null);
            });
          }}
        >
          {t("disk.compare")}
        </button>
      </div>,
    );
  }
  if (s.sharedWriter) {
    banners.push(
      <div className="banner info" key="shared">
        <span>{t("banner.sharedWriter")}</span>
      </div>,
    );
  }
  if (s.replaceProgress !== undefined) {
    banners.push(
      <div className="banner info" key="replace">
        <span>{t("find.replacing", { pct: Math.round(s.replaceProgress * 100) })}</span>
        <progress max={1} value={s.replaceProgress} />
        <button
          type="button"
          onClick={() => {
            void session.cancel();
          }}
        >
          {t("common.cancel")}
        </button>
      </div>,
    );
  }
  return (
    <>
      {banners}
      {hunks && (
        <div className="compare" role="dialog" aria-label={t("disk.compare")}>
          <div className="compare-head">
            <span>{t("disk.compareTitle", { count: hunks.length })}</span>
            <button
              type="button"
              onClick={() => {
                setHunks(null);
              }}
            >
              {t("common.close")}
            </button>
          </div>
          <div className="compare-body">
            {hunks.slice(0, 500).map((h, i) => (
              <div className="hunk" key={i}>
                <div className="hunk-line">{t("disk.atLine", { line: h.mineLines[0] + 1 })}</div>
                <pre className="mine">{h.mine}</pre>
                <pre className="disk">{h.disk}</pre>
              </div>
            ))}
          </div>
        </div>
      )}
    </>
  );
}

function FindPanel({ session, replace }: { session: Session; replace: boolean }) {
  const s = useDocState();
  const [q, setQ] = useState<Query>({
    pattern: "",
    regex: false,
    caseSensitive: false,
    wholeWord: false,
  });
  const [repl, setRepl] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  useEffect(() => {
    const id = setTimeout(() => {
      void session.search(q.pattern ? q : undefined);
    }, 150);
    return () => {
      clearTimeout(id);
    };
  }, [session, q]);
  useEffect(() => {
    session.setFindRepeat((backwards) => {
      if (q.pattern) void session.find(q, backwards);
    });
    return () => {
      session.setFindRepeat(undefined);
    };
  }, [session, q]);
  const close = () => {
    void session.search(undefined);
    setUi({ find: "off" });
    session.view?.focus();
  };
  const sr = s.search;
  const count = sr
    ? sr.complete
      ? t(sr.truncated ? "find.countTruncated" : "find.count", { count: fmt.format(sr.matches) })
      : t("find.searching", {
          count: fmt.format(sr.matches),
          pct: Math.floor((sr.bytesDone / Math.max(1, sr.bytesTotal)) * 100),
        })
    : "";
  const toggle = (k: "regex" | "caseSensitive" | "wholeWord", label: string, title: string) => (
    <button
      type="button"
      className={q[k] ? "toggle on" : "toggle"}
      aria-pressed={q[k]}
      title={title}
      onClick={() => {
        setQ({ ...q, [k]: !q[k] });
      }}
    >
      {label}
    </button>
  );
  return (
    <div className="find" role="search">
      <div className="find-row">
        <input
          ref={input}
          aria-label={t("find.find")}
          placeholder={t("find.find")}
          value={q.pattern}
          onChange={(e) => {
            setQ({ ...q, pattern: e.target.value });
          }}
          onKeyDown={(e) => {
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "h") {
              e.preventDefault();
              setUi({ find: "replace" });
            }
            if (e.key === "Enter") void session.find(q, e.shiftKey);
            if (e.key === "Escape") close();
          }}
        />
        {toggle("caseSensitive", "Aa", t("find.case"))}
        {toggle("wholeWord", "ab", t("find.word"))}
        {toggle("regex", ".*", t("find.regex"))}
        <span className="find-count" aria-live="polite">
          {count}
        </span>
        <button
          type="button"
          onClick={() => {
            void session.find(q, true);
          }}
          title={t("find.previous")}
        >
          ↑
        </button>
        <button
          type="button"
          onClick={() => {
            void session.find(q, false);
          }}
          title={t("find.next")}
        >
          ↓
        </button>
        <button
          type="button"
          onClick={() => {
            setUi({ find: replace ? "find" : "replace" });
          }}
          title={t("find.toggleReplace")}
        >
          ⇄
        </button>
        <button type="button" onClick={close} title={t("common.close")}>
          ×
        </button>
      </div>
      {replace && (
        <div className="find-row">
          <input
            aria-label={t("find.replace")}
            placeholder={t("find.replace")}
            value={repl}
            onChange={(e) => {
              setRepl(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void session.find(q, false, repl);
              if (e.key === "Escape") close();
            }}
          />
          <button
            type="button"
            disabled={!q.pattern}
            onClick={() => {
              void session.find(q, false, repl);
            }}
          >
            {t("find.replaceOne")}
          </button>
          <button
            type="button"
            disabled={!q.pattern}
            onClick={() => {
              void session.replaceAll(q, repl);
            }}
          >
            {t("find.replaceAll")}
          </button>
        </div>
      )}
    </div>
  );
}

function GotoLine({ session }: { session: Session }) {
  const s = useDocState();
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
  }, []);
  const total = s.status?.lineCount;
  const close = () => {
    setUi({ goto: false });
    session.view?.focus();
  };
  return (
    <div className="goto" role="dialog" aria-label={t("goto.title")}>
      <input
        ref={input}
        inputMode="numeric"
        aria-label={t("goto.title")}
        placeholder={
          total
            ? t(total.exact ? "goto.placeholder" : "goto.placeholderEstimate", {
                count: fmt.format(total.line),
              })
            : ""
        }
        value={value}
        onChange={(e) => {
          setValue(e.target.value.replace(/[^0-9]/g, ""));
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && value) {
            void session.gotoLine(Number(value)).then(close);
          }
          if (e.key === "Escape") close();
        }}
      />
      {total && !total.exact && <span className="hint">{t("goto.approximate")}</span>}
    </div>
  );
}

function StatusBar({
  session,
  wrap,
  onToggleWrap,
}: {
  session: Session;
  wrap: boolean;
  onToggleWrap: () => void;
}) {
  const s = useDocState();
  const [menu, setMenu] = useState<"encoding" | "eol" | null>(null);
  const st = s.status;
  if (!st) return <div className="statusbar" />;
  const c = s.caret;
  const pos = c
    ? c.line !== undefined
      ? t("status.position", { line: fmt.format(c.line), col: fmt.format(c.col) })
      : t("status.positionUnknown", { col: fmt.format(c.col) })
    : "";
  const lines = t(st.lineCount.exact ? "status.lines" : "status.linesEstimate", {
    count: fmt.format(st.lineCount.line),
  });
  const indexing = st.index.complete
    ? ""
    : t("status.indexing", {
        pct: Math.floor((st.index.knownBytes / Math.max(1, st.index.bytes)) * 100),
      });
  const ending = endingLabel(st.index.endings);
  const eolText = st.convertEol
    ? t("status.eolConvert", {
        from: ending || st.eol.toUpperCase(),
        to: st.convertEol.toUpperCase(),
      })
    : ending === "mixed"
      ? t("status.eolMixed")
      : ending || st.eol.toUpperCase();
  const save = s.saving ? t("status.saving") : st.dirty ? t("status.unsaved") : t("status.saved");
  return (
    <div className="statusbar" role="status">
      <span>{pos}</span>
      {c && c.selections > 1 && (
        <span>{t("status.selections", { count: fmt.format(c.selections) })}</span>
      )}
      {c && c.selections === 1 && c.selectedBytes > 0 && (
        <span>{t("status.selected", { size: bytes(c.selectedBytes) })}</span>
      )}
      <span className="spacer" />
      {indexing && <span className="progress">{indexing}</span>}
      <span>{lines}</span>
      <span>{bytes(st.len)}</span>
      <button type="button" className={wrap ? "on" : ""} onClick={onToggleWrap} title="Alt+Z">
        {t("status.wrap")}
      </button>
      <button
        type="button"
        className={ending === "mixed" ? "warn" : ""}
        onClick={() => {
          setMenu(menu === "eol" ? null : "eol");
        }}
      >
        {eolText}
      </button>
      <button
        type="button"
        onClick={() => {
          setMenu(menu === "encoding" ? null : "encoding");
        }}
        title={t("status.detectedBy", { by: st.detectedBy })}
      >
        {st.encoding}
        {st.bom ? " BOM" : ""}
      </button>
      {st.readOnly && <span className="badge">{t("status.readOnly")}</span>}
      <span className={st.dirty ? "dirty" : ""}>{save}</span>
      {menu === "eol" && (
        <div className="menu" role="menu">
          {(["lf", "crlf", "cr"] as const).map((le) => (
            <button
              type="button"
              role="menuitem"
              key={le}
              onClick={() => {
                setMenu(null);
                void session.setFormat(st.encodingLabel, st.bom, le);
              }}
            >
              {t("status.convertTo", { eol: le.toUpperCase() })}
            </button>
          ))}
          {st.convertEol && (
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setMenu(null);
                void session.setFormat(st.encodingLabel, st.bom, null);
              }}
            >
              {t("status.keepEndings")}
            </button>
          )}
        </div>
      )}
      {menu === "encoding" && (
        <div className="menu wide" role="menu">
          <div className="menu-head">{t("status.saveWith")}</div>
          {ENCODINGS.map((e) => (
            <button
              type="button"
              role="menuitem"
              key={`s-${e}`}
              onClick={() => {
                setMenu(null);
                void session.setFormat(e, e.startsWith("UTF") ? st.bom : false, st.convertEol);
              }}
            >
              {e}
            </button>
          ))}
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setMenu(null);
              void session.setFormat(st.encodingLabel, !st.bom, st.convertEol);
            }}
          >
            {st.bom ? t("status.removeBom") : t("status.addBom")}
          </button>
          <div className="menu-head">{t("status.reopenWith")}</div>
          {ENCODINGS.map((e) => (
            <button
              type="button"
              role="menuitem"
              key={`r-${e}`}
              onClick={() => {
                setMenu(null);
                void session.reopen(e);
              }}
            >
              {e}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
