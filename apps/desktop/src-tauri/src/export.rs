//! Markdown exports (M2 task 11, ADR-0031): self-contained HTML and PDF.
//!
//! The host renders the whole document (highlighted, sanitized, ADR-0028)
//! and inlines its images as `data:` URIs, through the same checks the
//! preview's image scheme makes (local files that are images, remote ones
//! only with the document's toggle on). The frontend adds math, diagrams and
//! the stylesheet and asks for the file to be written; for PDF the finished
//! HTML is printed by WebView2 in a hidden window. Whatever isn't carried
//! over is listed in the export's report.

use std::sync::Arc;

use base64::Engine as _;
use serde::Serialize;

use crate::images::ImageService;

/// Something the export left out or changed.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReportItem {
    pub kind: &'static str,
    pub source: String,
    pub reason: String,
}

/// Images over this size stay out of the exported file (reported).
const MAX_IMAGE: usize = 32 * 1024 * 1024;

/// Replaces `data-src="<prefix>…"` images with `src="data:…"`; images
/// that can't be inlined keep their alt text and are reported, as are
/// remote images left as placeholders (`data-blocked-src`).
pub fn inline_images(
    html: &str,
    prefix: &str,
    images: &Arc<ImageService>,
    report: &mut Vec<ReportItem>,
) -> String {
    const LAZY: &str = "data-src=\"";
    const BLOCKED: &str = "data-blocked-src=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let lazy = rest.find(LAZY);
        let blocked = rest.find(BLOCKED);
        let (at, is_lazy) = match (lazy, blocked) {
            (Some(a), Some(b)) if b < a => (b, false),
            (Some(a), _) => (a, true),
            (None, Some(b)) => (b, false),
            (None, None) => break,
        };
        let attr = if is_lazy { LAZY } else { BLOCKED };
        let start = at + attr.len();
        let Some(len) = rest[start..].find('"') else {
            break;
        };
        let value = unescape(&rest[start..start + len]);
        out.push_str(&rest[..at]);
        if is_lazy {
            match value
                .strip_prefix(prefix)
                .ok_or_else(|| "not an image of this document".to_owned())
                .and_then(|path| images.export_bytes(path))
            {
                Ok((bytes, mime)) if bytes.len() <= MAX_IMAGE => {
                    out.push_str("src=\"data:");
                    out.push_str(mime);
                    out.push_str(";base64,");
                    out.push_str(&base64::engine::general_purpose::STANDARD.encode(&*bytes));
                    out.push('"');
                }
                Ok(_) => {
                    report.push(ReportItem {
                        kind: "image",
                        source: shown_source(&value, prefix),
                        reason: "larger than 32 MiB".into(),
                    });
                    out.push_str(&rest[at..start + len + 1]);
                }
                Err(reason) => {
                    report.push(ReportItem {
                        kind: "image",
                        source: shown_source(&value, prefix),
                        reason,
                    });
                    out.push_str(&rest[at..start + len + 1]);
                }
            }
        } else {
            report.push(ReportItem {
                kind: "image",
                source: value.clone(),
                reason: "remote content is off for this document".into(),
            });
            out.push_str(&rest[at..start + len + 1]);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// The path or URL an image scheme URL stands for, for the report.
fn shown_source(url: &str, prefix: &str) -> String {
    let path = url.strip_prefix(prefix).unwrap_or(url);
    let enc = path
        .strip_prefix("local/")
        .or_else(|| {
            path.strip_prefix("remote/")
                .and_then(|r| r.split_once('/').map(|x| x.1))
        })
        .unwrap_or(path);
    quire_markdown::images::percent_decode(enc).unwrap_or_else(|| enc.to_owned())
}

/// Writes `bytes` to `path` atomically (a temporary file next to it, then a
/// rename).
pub fn write_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = path.with_file_name(format!(".{}.quire-tmp", name.to_string_lossy()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Prints `html` (a finished, self-contained page) to a PDF at `out` with
/// WebView2's PDF printer, in a hidden window that loads it from a
/// temporary file. Blocks until the PDF is written (at most 5 minutes).
#[cfg(windows)]
pub fn print_pdf<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    html: &[u8],
    out: &std::path::Path,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use tauri::webview::PageLoadEvent;
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("quire-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let page = dir.join(format!("page-{n}.html"));
    std::fs::write(&page, html).map_err(|e| e.to_string())?;
    let url = tauri::Url::from_file_path(&page).map_err(|()| "bad temporary path".to_owned())?;
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let target = out.to_owned();
    let started = std::sync::atomic::AtomicBool::new(false);
    let label = format!("export-{n}");
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url));
    // Every webview of the app shares one WebView2 environment, which
    // refuses a webview created with other browser arguments
    // (0x8007139F): under WebDriver the main window gets msedgedriver's.
    let builder = match crate::webdriver_browser_args() {
        Some(args) => builder.additional_browser_args(&args),
        None => builder,
    };
    let window = builder
        .visible(false)
        .on_page_load(move |w, payload| {
            if payload.event() != PageLoadEvent::Finished || started.swap(true, Ordering::AcqRel) {
                return;
            }
            let tx = tx.clone();
            let target = target.clone();
            let r = w.with_webview(move |wv| {
                // SAFETY: with_webview runs this on the webview's UI thread.
                if let Err(e) = unsafe { start_print(&wv, &target, tx.clone()) } {
                    let _ = tx.send(Err(e.to_string()));
                }
            });
            if let Err(e) = r {
                tracing::warn!(%e, "export window: with_webview failed");
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    let result = rx
        .recv_timeout(Duration::from_secs(300))
        .unwrap_or_else(|_| Err("the PDF printer didn't finish in 5 minutes".into()));
    let _ = window.destroy();
    let _ = std::fs::remove_file(&page);
    let _ = app.get_webview_window(&label).map(|w| w.destroy());
    result
}

/// # Safety
/// Must run on the webview's UI thread.
#[cfg(windows)]
unsafe fn start_print(
    wv: &tauri::webview::PlatformWebview,
    out: &std::path::Path,
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_7, ICoreWebView2Environment6,
    };
    use webview2_com::PrintToPdfCompletedHandler;
    use windows_core::{HSTRING, Interface};

    // SAFETY: COM calls on live interfaces owned by this webview, on its UI
    // thread.
    unsafe {
        let core: ICoreWebView2_7 = wv.controller().CoreWebView2()?.cast()?;
        let env: ICoreWebView2Environment6 = wv.environment().cast()?;
        let settings = env.CreatePrintSettings()?;
        settings.SetShouldPrintBackgrounds(true)?;
        settings.SetShouldPrintHeaderAndFooter(false)?;
        let handler = PrintToPdfCompletedHandler::create(Box::new(move |hr, ok| {
            let _ = tx.send(match hr {
                Ok(()) if ok => Ok(()),
                Ok(()) => Err("the PDF printer failed".into()),
                Err(e) => Err(e.to_string()),
            });
            Ok(())
        }));
        core.PrintToPdf(&HSTRING::from(out.as_os_str()), &settings, &handler)
    }
}

/// Attribute values come HTML-escaped from the renderer.
fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::memory::{GovernorConfig, MemoryGovernor};
    use quire_markdown::images::percent_encode;

    #[test]
    fn local_images_inline_and_the_rest_is_reported() {
        let dir = std::env::temp_dir().join(format!("quire-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let txt = dir.join("b.png");
        std::fs::write(&txt, b"not an image").unwrap();
        let svc = ImageService::new(&MemoryGovernor::new(GovernorConfig::default()));
        let prefix = "http://quire-img.localhost/";
        let html = format!(
            r#"<p><img data-src="{prefix}local/{a}" alt="A" /><img data-src="{prefix}local/{b}" alt="B" /><img data-blocked-src="https://e.com/x.png" alt="C" /></p>"#,
            a = percent_encode(&png.to_string_lossy()),
            b = percent_encode(&txt.to_string_lossy()),
        );
        let mut report = Vec::new();
        let out = inline_images(&html, prefix, &svc, &mut report);
        assert!(
            out.contains(r#"<img src="data:image/png;base64,iVBORw0KGgpyZXN0" alt="A" />"#),
            "{out}"
        );
        assert_eq!(report.len(), 2, "{report:?}");
        assert_eq!(report[0].source, txt.to_string_lossy());
        assert_eq!(report[1].source, "https://e.com/x.png");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
