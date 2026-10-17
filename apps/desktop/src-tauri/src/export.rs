//! Markdown exports (M2 task 11, ADR-0031): self-contained HTML and PDF.
//!
//! The host renders the whole document block by block (highlighted,
//! sanitized, ADR-0028) on an export thread and inlines its images as
//! `data:` URIs, through the same checks the preview's image scheme makes
//! (local files that are images, remote ones only with the document's
//! toggle on). The frontend takes the parts in batches, adds math, diagrams
//! and the stylesheet off its main thread and puts the page back in pieces
//! ([`ExportJob`]); the host writes it, or for PDF prints it with WebView2 in
//! a hidden window. Whatever isn't carried over is listed in the report.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use parking_lot::{Condvar, Mutex};
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

/// What a cancelled export's commands answer.
pub const CANCELLED: &str = "cancelled";

/// Bytes of parts the renderer may queue ahead of the frontend.
const QUEUE_MAX: usize = 8 * 1024 * 1024;

/// A piece of the finished page, put by the frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Head,
    Body,
    Tail,
}

/// An export being built (ADR-0031 addendum, RISKS #17). The core renders
/// the document block by block on its own thread and queues the parts
/// (images inlined); the frontend takes them in batches, adds math and
/// diagrams off its main thread and puts the page back in pieces; then the
/// page is written or printed. Nothing here holds the page twice, and
/// cancelling stops the renderer, the frontend's next take and a PDF print.
pub struct ExportJob {
    pub doc: u64,
    cancel: AtomicBool,
    state: Mutex<JobState>,
    changed: Condvar,
}

#[derive(Default)]
struct JobState {
    /// Rendered parts not yet taken, with the source offset each reaches.
    parts: VecDeque<(String, u64)>,
    queued: usize,
    rendered: bool,
    error: Option<String>,
    report: Vec<ReportItem>,
    head: Vec<u8>,
    body: Vec<Vec<u8>>,
    tail: Vec<u8>,
}

impl ExportJob {
    pub fn new(doc: u64) -> Self {
        Self {
            doc,
            cancel: AtomicBool::new(false),
            state: Mutex::new(JobState::default()),
            changed: Condvar::new(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        let _s = self.state.lock();
        self.changed.notify_all();
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// The flag a PDF print watches.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    /// Renderer: queues a part, waiting while the queue is full. False once
    /// the export is cancelled (the renderer stops).
    pub fn push(&self, html: String, offset: u64, report: Vec<ReportItem>) -> bool {
        let mut s = self.state.lock();
        while s.queued >= QUEUE_MAX && !self.cancelled() {
            self.changed.wait(&mut s);
        }
        if self.cancelled() {
            return false;
        }
        s.queued += html.len();
        s.parts.push_back((html, offset));
        s.report.extend(report);
        self.changed.notify_all();
        true
    }

    /// Renderer: all parts are queued (or rendering failed).
    pub fn rendered(&self, error: Option<String>) {
        let mut s = self.state.lock();
        s.rendered = true;
        s.error = error;
        self.changed.notify_all();
    }

    /// Frontend: the next batch, whole parts up to `max` bytes (at least
    /// one), waiting for the renderer: the source offset reached (u64 LE)
    /// followed by the HTML. Just the offset once every part was taken.
    pub fn take(&self, max: usize) -> Result<Vec<u8>, String> {
        let mut s = self.state.lock();
        while s.parts.is_empty() && !s.rendered && !self.cancelled() {
            self.changed.wait(&mut s);
        }
        if self.cancelled() {
            return Err(CANCELLED.into());
        }
        if let Some(e) = s.error.take() {
            return Err(e);
        }
        let mut out = vec![0; 8];
        let mut offset = 0;
        while let Some((html, _)) = s.parts.front() {
            if out.len() > 8 && out.len() - 8 + html.len() > max {
                break;
            }
            let (html, off) = s.parts.pop_front().unwrap_or_default();
            s.queued -= html.len();
            out.extend_from_slice(html.as_bytes());
            offset = off;
        }
        if out.len() > 8 {
            out[..8].copy_from_slice(&offset.to_le_bytes());
        }
        self.changed.notify_all();
        Ok(out)
    }

    /// Frontend: a piece of the finished page (the body comes in order).
    pub fn put(&self, piece: Piece, bytes: Vec<u8>) -> Result<(), String> {
        if self.cancelled() {
            return Err(CANCELLED.into());
        }
        let mut s = self.state.lock();
        match piece {
            Piece::Head => s.head = bytes,
            Piece::Body => s.body.push(bytes),
            Piece::Tail => s.tail = bytes,
        }
        Ok(())
    }

    /// The page's size so far and the report of what was left out.
    pub fn summary(&self) -> (usize, Vec<ReportItem>) {
        let s = self.state.lock();
        let len = s.head.len() + s.body.iter().map(Vec::len).sum::<usize>() + s.tail.len();
        (len, s.report.clone())
    }

    /// The page's pieces in order, taken out of the job.
    pub fn take_page(&self) -> Vec<Vec<u8>> {
        let mut s = self.state.lock();
        let mut out = Vec::with_capacity(s.body.len() + 2);
        out.push(std::mem::take(&mut s.head));
        out.append(&mut s.body);
        out.push(std::mem::take(&mut s.tail));
        out
    }
}

/// Writes `pieces` to `path` atomically (a temporary file next to it, then
/// a rename).
pub fn write_file<B: AsRef<[u8]>>(path: &std::path::Path, pieces: &[B]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = sibling_tmp(path)?;
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        for p in pieces {
            f.write_all(p.as_ref())?;
        }
        f.into_inner()
            .map_err(std::io::IntoInnerError::into_error)?
            .sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn sibling_tmp(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    Ok(path.with_file_name(format!(".{}.quire-tmp", name.to_string_lossy())))
}

/// Moves a finished temporary file to `path` (a copy next to it first when
/// they are on different volumes).
#[cfg(windows)]
fn move_into_place(from: &std::path::Path, path: &std::path::Path) -> std::io::Result<()> {
    if std::fs::rename(from, path).is_ok() {
        return Ok(());
    }
    let tmp = sibling_tmp(path)?;
    let r = std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    let _ = std::fs::remove_file(from);
    r
}

/// Prints `page` (a finished, self-contained page, in pieces) to a PDF at
/// `out` with WebView2's PDF printer, in a hidden window that loads it from
/// a temporary file. The PDF is written next to the page and moved into
/// place when it is complete. Blocks until then, at most 5 minutes, or
/// until `cancel` is set.
#[cfg(windows)]
pub fn print_pdf<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    page_pieces: &[Vec<u8>],
    out: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    use tauri::webview::PageLoadEvent;
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("quire-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let page = dir.join(format!("page-{n}.html"));
    write_file(&page, page_pieces).map_err(|e| e.to_string())?;
    let printed = dir.join(format!("page-{n}.pdf"));
    let url = tauri::Url::from_file_path(&page).map_err(|()| "bad temporary path".to_owned())?;
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let target = printed.clone();
    let started = AtomicBool::new(false);
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
    let deadline = Instant::now() + Duration::from_secs(300);
    let result = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => break r,
            Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::Acquire) => {
                break Err(CANCELLED.into());
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(RecvTimeoutError::Timeout) => {
                break Err("the PDF printer didn't finish in 5 minutes".into());
            }
            Err(RecvTimeoutError::Disconnected) => break Err("the PDF printer stopped".into()),
        }
    };
    let _ = window.destroy();
    let _ = std::fs::remove_file(&page);
    let _ = app.get_webview_window(&label).map(|w| w.destroy());
    let result = result.and_then(|()| move_into_place(&printed, out).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&printed);
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
    fn export_job_hands_out_whole_parts_and_stops_when_cancelled() {
        let job = Arc::new(ExportJob::new(7));
        let renderer = Arc::clone(&job);
        let t = std::thread::spawn(move || {
            assert!(renderer.push("<p>a</p>".into(), 4, Vec::new()));
            assert!(renderer.push("<p>bb</p>".into(), 9, Vec::new()));
            assert!(renderer.push("x".repeat(20), 30, Vec::new()));
            renderer.rendered(None);
        });
        t.join().unwrap();
        // Whole parts up to the limit, the offset reached first.
        let b = job.take(17).unwrap();
        assert_eq!(u64::from_le_bytes(b[..8].try_into().unwrap()), 9);
        assert_eq!(&b[8..], b"<p>a</p><p>bb</p>");
        // A part over the limit still comes, alone.
        let b = job.take(4).unwrap();
        assert_eq!(u64::from_le_bytes(b[..8].try_into().unwrap()), 30);
        assert_eq!(b.len(), 28);
        // Then just the offset: the end.
        assert_eq!(job.take(4).unwrap().len(), 8);
        job.put(Piece::Body, b"<p>".to_vec()).unwrap();
        job.put(Piece::Tail, b"!".to_vec()).unwrap();
        job.put(Piece::Head, b"<h>".to_vec()).unwrap();
        assert_eq!(job.summary().0, 7);
        assert_eq!(job.take_page().concat(), b"<h><p>!");
        job.cancel();
        assert!(!job.push("late".into(), 40, Vec::new()));
        assert_eq!(job.take(4), Err(CANCELLED.to_owned()));
        assert!(job.put(Piece::Body, Vec::new()).is_err());
    }

    #[test]
    fn a_full_queue_waits_until_cancelled() {
        let job = Arc::new(ExportJob::new(1));
        assert!(job.push("x".repeat(QUEUE_MAX), 1, Vec::new()));
        let renderer = Arc::clone(&job);
        let t = std::thread::spawn(move || renderer.push("y".into(), 2, Vec::new()));
        std::thread::sleep(std::time::Duration::from_millis(50));
        job.cancel();
        assert!(!t.join().unwrap());
    }

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
