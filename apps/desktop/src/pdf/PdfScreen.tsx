// Chrome around the PDF view (M3 task 5): toolbar, password prompt,
// "repaired" banner, status bar. The pages themselves are PdfView's.

import { useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import type { LayoutMode } from "./layout";
import { PdfView, type PdfOpened, type PdfViewState } from "./view";

const ZOOMS = [25, 50, 75, 100, 125, 150, 200, 300, 400, 800, 1600, 3200, 6400];
const fmt = new Intl.NumberFormat();

/** The open PDF view, for UI tests and benchmarks (WebDriver only). */
export const pdfState: { view: PdfView | undefined; opened: PdfOpened | undefined } = {
  view: undefined,
  opened: undefined,
};

export function PdfScreen({ path, onError }: { path: string; onError: (e: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<PdfView | undefined>(undefined);
  const [state, setState] = useState<PdfViewState | undefined>();
  const [opened, setOpened] = useState<PdfOpened | undefined>();
  const [needPassword, setNeedPassword] = useState<"first" | "wrong" | undefined>();
  const [password, setPassword] = useState("");
  const [pageInput, setPageInput] = useState<string | undefined>();

  const tryOpen = (pw?: string) => {
    const view = viewRef.current;
    if (!view) return;
    view
      .open(path, pw)
      .then((info) => {
        setNeedPassword(undefined);
        setOpened(info);
        pdfState.opened = info;
        view.element.focus();
      })
      .catch((err: unknown) => {
        if (String(err) === "password") setNeedPassword(pw === undefined ? "first" : "wrong");
        else onError(String(err));
      });
  };

  useEffect(() => {
    if (!host.current) return;
    const view = new PdfView(host.current);
    viewRef.current = view;
    pdfState.view = view;
    const off = view.onChange(setState);
    tryOpen();
    return () => {
      off();
      pdfState.view = undefined;
      pdfState.opened = undefined;
      void view.close();
    };
    // One view per path; App keys this screen by path.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  const view = viewRef.current;
  const name = path.split(/[\\/]/).pop() ?? path;
  return (
    <div className="editor pdf-screen">
      <div className="toolbar" role="toolbar" aria-label={t("pdf.toolbar")}>
        <span className="doc-name" title={path}>
          {name}
        </span>
        <span className="spacer" />
        <button
          type="button"
          aria-label={t("pdf.zoomOut")}
          onClick={() => view?.zoomBy(0.8)}
          disabled={!opened}
        >
          −
        </button>
        <select
          aria-label={t("pdf.zoom")}
          disabled={!opened}
          value={state?.zoomMode === "custom" ? "custom" : (state?.zoomMode ?? "fitWidth")}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "fitWidth" || v === "fitPage") view?.fit(v);
            else if (v !== "custom") view?.setZoom(Number(v));
          }}
        >
          <option value="fitWidth">{t("pdf.fitWidth")}</option>
          <option value="fitPage">{t("pdf.fitPage")}</option>
          {state?.zoomMode === "custom" && (
            <option value="custom">{`${fmt.format(Math.round(state.zoom))} %`}</option>
          )}
          {ZOOMS.map((z) => (
            <option key={z} value={z}>{`${fmt.format(z)} %`}</option>
          ))}
        </select>
        <button
          type="button"
          aria-label={t("pdf.zoomIn")}
          onClick={() => view?.zoomBy(1.25)}
          disabled={!opened}
        >
          +
        </button>
        <select
          aria-label={t("pdf.layout")}
          disabled={!opened}
          value={state?.mode ?? "continuous"}
          onChange={(e) => view?.setMode(e.target.value as LayoutMode)}
        >
          <option value="continuous">{t("pdf.continuous")}</option>
          <option value="single">{t("pdf.single")}</option>
          <option value="twoUp">{t("pdf.twoUp")}</option>
        </select>
        <button
          type="button"
          aria-label={t("pdf.rotateLeft")}
          onClick={() => view?.rotate(-90)}
          disabled={!opened}
        >
          ⟲
        </button>
        <button
          type="button"
          aria-label={t("pdf.rotateRight")}
          onClick={() => view?.rotate(90)}
          disabled={!opened}
        >
          ⟳
        </button>
      </div>
      {opened && !opened.validXref && (
        <div className="banner" role="status">
          <span>{t("pdf.repaired")}</span>
        </div>
      )}
      {needPassword && (
        <form
          className="banner pdf-password"
          onSubmit={(e) => {
            e.preventDefault();
            tryOpen(password);
          }}
        >
          <label>
            {needPassword === "wrong" ? t("pdf.passwordWrong") : t("pdf.passwordNeeded")}{" "}
            <input
              type="password"
              autoFocus
              value={password}
              onChange={(e) => {
                setPassword(e.target.value);
              }}
            />
          </label>
          <button type="submit">{t("pdf.unlock")}</button>
        </form>
      )}
      <div className="editor-surface" ref={host} />
      <div className="statusbar" role="status">
        {state && state.pages > 0 && (
          <form
            className="pdf-page"
            onSubmit={(e) => {
              e.preventDefault();
              const n = Number(pageInput);
              if (Number.isFinite(n)) view?.gotoPage(n - 1);
              setPageInput(undefined);
              view?.element.focus();
            }}
          >
            <label>
              {t("pdf.page")}{" "}
              <input
                aria-label={t("pdf.goToPage")}
                size={5}
                value={pageInput ?? String(state.page + 1)}
                onFocus={(e) => {
                  e.target.select();
                }}
                onChange={(e) => {
                  setPageInput(e.target.value);
                }}
                onBlur={() => {
                  setPageInput(undefined);
                }}
              />{" "}
              {t("pdf.ofPages", { count: fmt.format(state.pages) })}
            </label>
          </form>
        )}
        {state?.estimated && <span>{t("pdf.estimating")}</span>}
        {state && state.failedPages > 0 && (
          <span className="warn">{t("pdf.failedPages", { count: state.failedPages })}</span>
        )}
        <span className="spacer" />
        {state && <span>{`${fmt.format(Math.round(state.zoom))} %`}</span>}
        {opened && <span>{`PDF ${Math.floor(opened.version / 10)}.${opened.version % 10}`}</span>}
      </div>
    </div>
  );
}
