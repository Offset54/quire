//! TXT documents in the host: one session per open document, the commands
//! the frontend calls, and the background work around them (line index,
//! search, replace-all, journal, file watching).
//!
//! - Bulk text goes out as binary view messages (`quire_text::view`) and
//!   comes in as raw request bodies (paste); only small control data is JSON.
//! - Edits run on the command thread under the document lock: an edit is a
//!   few tree operations, far below a frame. Long work (index, search,
//!   replace-all, the write phase of a save) runs from snapshots, off the lock.
//! - A file given on the command line starts opening before the webview
//!   exists (ADR-0022), and `txt_startup` hands it to the frontend.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use parking_lot::{Condvar, Mutex};
use quire_core::memory::{DocId, MemoryGovernor, Registration};
use quire_core::scheduler::{CancelToken, Scheduler};
use quire_text::document::{Action, LineEnding, SaveFormat};
use quire_text::file::{
    OnUnmappable, OpenSettings, SaveError, SaveOptions, SaveReport, TextFile, commit_save,
    prepare_save,
};
use quire_text::index::index_in_background;
use quire_text::journal::{self, Header, JournalWriter};
use quire_text::search::{self, Matcher, Query, SearchProgress};
use quire_text::view::{self, ViewRequest};
use quire_text::watch::{self, DiskStatus, EditLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::ipc::{Channel, Request, Response};
use tauri::{AppHandle, State};

/// Where journals and temp buffers live: the app's local data directory.
pub fn data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("dev.quire.app")
}

fn threads() -> usize {
    quire_core::scheduler::SchedulerConfig::default().threads
}

/// Search state of one document.
#[derive(Default)]
struct SearchState {
    generation: u64,
    matcher: Option<Matcher>,
    matches: Vec<(u64, u64)>,
    progress: SearchProgress,
    /// Revision the matches belong to; stale after an edit.
    revision: u64,
    cancel: Option<CancelToken>,
}

pub struct Session {
    pub id: u64,
    file: Mutex<TextFile>,
    journal: Mutex<Option<JournalWriter>>,
    events: Mutex<Option<Channel<Value>>>,
    search: Mutex<SearchState>,
    /// A save or replace-all is running: edits are refused meanwhile.
    busy: AtomicBool,
    index_cancel: Mutex<CancelToken>,
    add_bytes: Registration,
    lock_warned: AtomicBool,
}

impl Session {
    fn emit(&self, v: Value) {
        if let Some(c) = self.events.lock().as_ref()
            && let Err(err) = c.send(v)
        {
            tracing::debug!(%err, "document event dropped");
        }
    }

    fn status(&self, f: &TextFile) -> Value {
        let snap = f.doc.snapshot();
        let p = snap
            .buffers
            .get(quire_text::buffer::ORIGINAL)
            .index
            .progress();
        json!({
            "kind": "status",
            "doc": self.id,
            "path": f.path,
            "len": snap.len(),
            "lineCount": snap.line_count_estimate(),
            "index": p,
            "encoding": quire_text::encoding::display_name(f.doc.format.encoding),
            "encodingLabel": f.doc.format.encoding.name(),
            "detectedBy": f.detection.by,
            "bom": f.doc.format.bom,
            "eol": f.doc.eol,
            "convertEol": f.doc.format.line_ending,
            "dirty": f.doc.is_dirty(),
            "readOnly": f.doc.read_only,
            "binary": f.detection.binary,
            "source": f.source_kind,
            "remote": f.remote,
            "revision": f.doc.revision(),
            "busy": self.busy.load(Ordering::Acquire),
        })
    }

    /// After a change: journal it, hold or release the edit lock, report.
    fn after_change(&self, f: &mut TextFile) {
        let records = f.doc.take_records();
        if !records.is_empty() {
            let mut j = self.journal.lock();
            if j.is_none() {
                let path = journal::journal_name(f.path.as_deref());
                match JournalWriter::create(&data_dir().join("journal").join(path), &header(f)) {
                    Ok(w) => *j = Some(w),
                    Err(err) => tracing::error!(%err, "journal could not be created"),
                }
            }
            if let Some(w) = j.as_ref() {
                w.append(records);
            }
        }
        let dirty = f.doc.is_dirty();
        if dirty
            && f.edit_lock.is_none()
            && let Some(p) = f.path.clone()
        {
            match EditLock::acquire(&p) {
                Ok(l) => f.edit_lock = Some(l),
                Err(err) => {
                    if !self.lock_warned.swap(true, Ordering::AcqRel) {
                        tracing::warn!(%err, "another program has the file open for writing");
                        self.emit(json!({"kind": "sharedWriter", "doc": self.id}));
                    }
                }
            }
        } else if !dirty {
            f.edit_lock = None;
        }
        self.add_bytes
            .charge(f.doc.buffer().add_bytes() as i64 - self.add_bytes.bytes() as i64);
    }
}

fn header(f: &TextFile) -> Header {
    Header {
        path: f.path.clone(),
        stamp: f.stamp,
        encoding: f.detection.encoding.name().to_owned(),
        created_ms: journal::now_ms(),
        pid: std::process::id(),
    }
}

/// A command-line file being opened before the webview is ready.
#[derive(Default)]
struct Startup {
    state: Mutex<Option<Result<u64, String>>>,
    requested: AtomicBool,
    ready: Condvar,
}

pub struct TxtService {
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    scheduler: Arc<Scheduler>,
    governor: MemoryGovernor,
    next_id: AtomicU64,
    startup: Startup,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    watched: Mutex<HashMap<PathBuf, usize>>,
    weak: Mutex<Weak<TxtService>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInfo {
    pub doc: u64,
    pub status: Value,
    pub open_ms: f64,
}

type CmdResult<T> = Result<T, String>;

/// Runs `f`, turning panics into errors (§8: isolate panics at the
/// provider boundary).
fn guarded<T>(f: impl FnOnce() -> CmdResult<T>) -> CmdResult<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        tracing::error!(%msg, "TXT command panicked");
        Err(format!("internal error: {msg}"))
    })
}

fn io_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl TxtService {
    pub fn new(scheduler: Arc<Scheduler>, governor: MemoryGovernor) -> Arc<Self> {
        let s = Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            scheduler,
            governor,
            next_id: AtomicU64::new(1),
            startup: Startup::default(),
            watcher: Mutex::new(None),
            watched: Mutex::new(HashMap::new()),
            weak: Mutex::new(Weak::new()),
        });
        *s.weak.lock() = Arc::downgrade(&s);
        s.start_watcher();
        let _ = std::fs::create_dir_all(data_dir().join("tmp"));
        s
    }

    fn session(&self, doc: u64) -> CmdResult<Arc<Session>> {
        self.sessions
            .lock()
            .get(&doc)
            .cloned()
            .ok_or_else(|| format!("no document {doc}"))
    }

    fn settings(encoding: Option<&'static encoding_rs::Encoding>) -> OpenSettings<'static> {
        OpenSettings {
            encoding,
            temp_dir: data_dir().join("tmp"),
            threads: threads(),
            progress: &|_| true,
        }
    }

    /// Opens a file and starts indexing it; returns the session id.
    pub fn open(
        &self,
        path: &Path,
        encoding: Option<&'static encoding_rs::Encoding>,
    ) -> CmdResult<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _span = tracing::info_span!("txt_open", doc = id).entered();
        let file = quire_text::file::open(
            path,
            &self.governor,
            Some(DocId(id)),
            &Self::settings(encoding),
        )
        .map_err(io_err)?;
        Ok(self.adopt(id, file))
    }

    fn adopt(&self, id: u64, file: TextFile) -> u64 {
        let path = file.path.clone();
        let original = file.original_index();
        let session = Arc::new(Session {
            id,
            file: Mutex::new(file),
            journal: Mutex::new(None),
            events: Mutex::new(None),
            search: Mutex::new(SearchState::default()),
            busy: AtomicBool::new(false),
            index_cancel: Mutex::new(CancelToken::new()),
            add_bytes: self
                .governor
                .register_fixed("txt-add-buffer", Some(DocId(id))),
            lock_warned: AtomicBool::new(false),
        });
        self.sessions.lock().insert(id, Arc::clone(&session));
        if let Some(p) = &path {
            self.watch(p);
        }
        self.start_index(&session, original);
        id
    }

    /// Indexes buffer 0 in the background if it isn't yet; each finished
    /// job refreshes the tree's counts and reports progress.
    fn start_index(&self, session: &Arc<Session>, original: quire_text::buffer::BufferEntry) {
        if original.index.is_complete() {
            return;
        }
        let cancel = CancelToken::new();
        let old = std::mem::replace(&mut *session.index_cancel.lock(), cancel.clone());
        old.cancel();
        let weak = Arc::downgrade(session);
        let started = Instant::now();
        drop(index_in_background(
            &self.scheduler,
            original.source,
            original.index,
            cancel,
            Arc::new(move |index| {
                let Some(s) = weak.upgrade() else { return };
                let mut f = s.file.lock();
                f.doc.buffer_mut().refresh_index();
                let mut st = s.status(&f);
                if index.is_complete() {
                    st["indexMs"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                    tracing::info!(
                        doc = s.id,
                        ms = started.elapsed().as_millis(),
                        "line index complete"
                    );
                }
                drop(f);
                s.emit(st);
            }),
        ));
    }

    fn close_session(&self, s: &Arc<Session>, keep_journal: bool) {
        s.index_cancel.lock().cancel();
        if let Some(c) = s.search.lock().cancel.take() {
            c.cancel();
        }
        if let Some(w) = s.journal.lock().take() {
            w.close(!keep_journal);
        }
        if let Some(p) = s.file.lock().path.clone() {
            self.unwatch(&p);
        }
        self.sessions.lock().remove(&s.id);
        self.governor.remove_document(DocId(s.id));
    }

    // ---- startup overlap (ADR-0022) ----------------------------------------

    /// Starts opening `path` on a thread right away (before the webview).
    pub fn preopen(self: &Arc<Self>, path: PathBuf) {
        self.startup.requested.store(true, Ordering::Release);
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("quire-preopen".into())
            .spawn(move || {
                let result = me.open(&path, None);
                // Lay out the first screen while the webview starts.
                if let Ok(id) = result
                    && let Ok(s) = me.session(id)
                {
                    let f = s.file.lock();
                    let _ = view::build(
                        &f.doc,
                        &ViewRequest {
                            anchor: 0,
                            caret: false,
                            before: 0,
                            after: 80,
                            layout: f.doc.layout,
                            hcols: Some((0, 240)),
                        },
                        &|_, _| Vec::new(),
                    );
                }
                *me.startup.state.lock() = Some(result);
                me.startup.ready.notify_all();
            })
            .expect("spawn preopen");
    }

    fn startup_doc(&self) -> Option<CmdResult<u64>> {
        if !self.startup.requested.load(Ordering::Acquire) {
            return None;
        }
        let mut st = self.startup.state.lock();
        while st.is_none() {
            self.startup.ready.wait(&mut st);
        }
        st.take()
    }

    // ---- file watching --------------------------------------------------------

    fn start_watcher(&self) {
        let weak = self.weak.lock().clone();
        let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let Some(svc) = weak.upgrade() else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            for p in ev.paths {
                svc.on_path_event(&p);
            }
        });
        match w {
            Ok(w) => *self.watcher.lock() = Some(w),
            Err(err) => tracing::warn!(%err, "file watcher unavailable; checking on focus only"),
        }
    }

    fn watch(&self, path: &Path) {
        use notify::Watcher;
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let mut watched = self.watched.lock();
        let n = watched.entry(dir.clone()).or_insert(0);
        *n += 1;
        if *n == 1
            && let Some(w) = self.watcher.lock().as_mut()
            && let Err(err) = w.watch(&dir, notify::RecursiveMode::NonRecursive)
        {
            tracing::warn!(%err, dir = %dir.display(), "could not watch directory");
        }
    }

    fn unwatch(&self, path: &Path) {
        use notify::Watcher;
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let mut watched = self.watched.lock();
        if let Some(n) = watched.get_mut(&dir) {
            *n -= 1;
            if *n == 0 {
                watched.remove(&dir);
                if let Some(w) = self.watcher.lock().as_mut() {
                    let _ = w.unwatch(&dir);
                }
            }
        }
    }

    fn on_path_event(&self, p: &Path) {
        let sessions: Vec<Arc<Session>> = self.sessions.lock().values().cloned().collect();
        for s in sessions {
            let matches = s
                .file
                .try_lock()
                .and_then(|f| f.path.as_ref().map(|q| q == p))
                .unwrap_or(false);
            if matches {
                let s2 = Arc::clone(&s);
                let svc = self.weak.lock().upgrade();
                std::thread::spawn(move || {
                    if let Some(svc) = svc {
                        let _ = svc.check_disk(&s2);
                    }
                });
            }
        }
    }

    /// Checks the file on disk; reloads a clean document automatically,
    /// otherwise reports the change to the frontend.
    fn check_disk(&self, s: &Arc<Session>) -> CmdResult<Option<DiskStatus>> {
        if s.busy.load(Ordering::Acquire) {
            return Ok(None);
        }
        let f = s.file.lock();
        let Some(now) = watch::stamp_changed(&f).map_err(io_err)? else {
            return Ok(None);
        };
        let status = watch::classify(&f, now).map_err(io_err)?;
        drop(f);
        if status.change == watch::DiskChange::Touched {
            s.file.lock().stamp = status.stamp;
            return Ok(None);
        }
        let dirty = s.file.lock().doc.is_dirty();
        if !dirty && status.change != watch::DiskChange::Deleted {
            self.reload(s, None)
                .map_err(|e| format!("reload failed: {e}"))?;
            s.emit(json!({"kind": "reloaded", "doc": s.id, "change": status.change}));
            return Ok(Some(status));
        }
        s.emit(json!({"kind": "disk", "doc": s.id, "status": status}));
        Ok(Some(status))
    }

    /// Re-reads the file into the session, keeping the caret near where it was.
    fn reload(
        &self,
        s: &Arc<Session>,
        encoding: Option<&'static encoding_rs::Encoding>,
    ) -> CmdResult<()> {
        let (path, caret, layout) = {
            let f = s.file.lock();
            (f.path.clone(), f.doc.primary().head, f.doc.layout)
        };
        let path = path.ok_or("untitled documents can't be reloaded")?;
        s.index_cancel.lock().cancel();
        let mut new = quire_text::file::open(
            &path,
            &self.governor,
            Some(DocId(s.id)),
            &Self::settings(encoding),
        )
        .map_err(io_err)?;
        new.doc.layout = layout;
        let caret = caret.min(new.doc.len());
        let start = new
            .doc
            .snapshot()
            .line_start_containing(caret)
            .map_err(io_err)?;
        new.doc
            .set_selections(vec![quire_text::document::Selection::caret(start)], 0);
        let original = new.original_index();
        *s.file.lock() = new;
        if let Some(w) = s.journal.lock().take() {
            w.close(true);
        }
        self.start_index(s, original);
        let st = s.status(&s.file.lock());
        s.emit(st);
        Ok(())
    }
}

#[cfg(feature = "bench-fixtures")]
impl TxtService {
    /// Places a caret at `offset` (snapped to a character) and types
    /// `text`; returns the core time of the typed edit in microseconds.
    pub fn bench_insert(&self, doc: u64, offset: u64, text: &str) -> CmdResult<f64> {
        let s = self.session(doc)?;
        let mut f = s.file.lock();
        let at = quire_text::document::snap_offset(&f.doc.snapshot(), offset).map_err(io_err)?;
        f.doc
            .apply(&Action::SetSelections {
                ranges: vec![(at, at)],
            })
            .map_err(io_err)?;
        let started = Instant::now();
        f.doc
            .apply(&Action::Type {
                text: text.to_owned(),
            })
            .map_err(io_err)?;
        let us = started.elapsed().as_secs_f64() * 1e6;
        s.after_change(&mut f);
        Ok(us)
    }
}

fn view_message(s: &Session, f: &TextFile, req: &ViewRequest) -> CmdResult<Response> {
    let search = s.search.lock();
    let fresh = search.revision == f.doc.revision();
    let matches = &search.matches;
    let msg = view::build(&f.doc, req, &|from, to| {
        if !fresh {
            return Vec::new();
        }
        let i = matches.partition_point(|m| m.1 <= from);
        matches[i..]
            .iter()
            .take_while(|m| m.0 < to)
            .take(10_000)
            .copied()
            .collect()
    })
    .map_err(io_err)?;
    Ok(Response::new(msg))
}

// ---- commands ----------------------------------------------------------------

#[tauri::command]
pub async fn txt_startup(
    svc: State<'_, Arc<TxtService>>,
    channel: Channel<Value>,
) -> CmdResult<Option<DocInfo>> {
    let started = Instant::now();
    let Some(result) = svc.startup_doc() else {
        return Ok(None);
    };
    let id = result?;
    let s = svc.session(id)?;
    *s.events.lock() = Some(channel);
    let status = s.status(&s.file.lock());
    Ok(Some(DocInfo {
        doc: id,
        status,
        open_ms: started.elapsed().as_secs_f64() * 1000.0,
    }))
}

#[tauri::command]
pub async fn txt_open(
    svc: State<'_, Arc<TxtService>>,
    path: String,
    encoding: Option<String>,
    channel: Channel<Value>,
) -> CmdResult<DocInfo> {
    guarded(|| {
        let started = Instant::now();
        let enc = encoding.and_then(|e| encoding_rs::Encoding::for_label(e.as_bytes()));
        let id = svc.open(Path::new(&path), enc)?;
        let s = svc.session(id)?;
        *s.events.lock() = Some(channel);
        let status = s.status(&s.file.lock());
        Ok(DocInfo {
            doc: id,
            status,
            open_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    })
}

/// A new, untitled document.
#[tauri::command]
pub async fn txt_new(
    svc: State<'_, Arc<TxtService>>,
    channel: Channel<Value>,
) -> CmdResult<DocInfo> {
    let id = svc.next_id.fetch_add(1, Ordering::Relaxed);
    let file = TextFile::untitled(&svc.governor, data_dir().join("tmp"));
    svc.adopt(id, file);
    let s = svc.session(id)?;
    *s.events.lock() = Some(channel);
    let status = s.status(&s.file.lock());
    Ok(DocInfo {
        doc: id,
        status,
        open_ms: 0.0,
    })
}

#[tauri::command]
pub async fn txt_close(svc: State<'_, Arc<TxtService>>, doc: u64, discard: bool) -> CmdResult<()> {
    let s = svc.session(doc)?;
    let dirty = s.file.lock().doc.is_dirty();
    svc.close_session(&s, dirty && !discard);
    Ok(())
}

#[tauri::command]
pub async fn txt_view(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    req: ViewRequest,
) -> CmdResult<Response> {
    guarded(|| {
        let s = svc.session(doc)?;
        let mut f = s.file.lock();
        f.doc.layout = req.layout;
        view_message(&s, &f, &req)
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub rows: u32,
}

#[tauri::command]
pub async fn txt_edit(
    app: AppHandle,
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    action: Action,
    req: ViewRequest,
    page: Option<PageInfo>,
) -> CmdResult<Response> {
    guarded(|| {
        let s = svc.session(doc)?;
        let mut f = s.file.lock();
        let mutating = !matches!(
            action,
            Action::Move { .. }
                | Action::SelectAll
                | Action::SelectLine
                | Action::AddCursorAbove
                | Action::AddCursorBelow
                | Action::AddNextOccurrence
                | Action::Collapse
                | Action::Click { .. }
                | Action::ColumnSelect { .. }
                | Action::SetSelections { .. }
        );
        if mutating && s.busy.load(Ordering::Acquire) {
            return Err("busy: the document is being saved or replaced".into());
        }
        if mutating && f.doc.read_only {
            return Err("read-only".into());
        }
        f.doc.layout = req.layout;
        if let Some(p) = page {
            f.doc.page_rows = p.rows.max(1);
        }
        let started = Instant::now();
        let out = f.doc.apply(&action).map_err(io_err)?;
        let apply_us = started.elapsed().as_micros();
        if let Some(text) = out.clipboard {
            write_clipboard(&app, text);
        }
        if out.changed {
            s.after_change(&mut f);
            tracing::trace!(apply_us, "txt edit");
        }
        view_message(&s, &f, &req)
    })
}

/// Pasted text arrives as the raw request body (§5.6: no bulk JSON); the
/// document id and view request are headers.
#[tauri::command]
pub async fn txt_paste(
    svc: State<'_, Arc<TxtService>>,
    request: Request<'_>,
) -> CmdResult<Response> {
    guarded(|| {
        let tauri::ipc::InvokeBody::Raw(body) = request.body() else {
            return Err("expected a raw body".into());
        };
        let header = |k: &str| {
            request
                .headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| format!("missing header {k}"))
        };
        let doc: u64 = header("x-quire-doc")?.parse().map_err(io_err)?;
        let req: ViewRequest = serde_json::from_str(&header("x-quire-view")?).map_err(io_err)?;
        let text = String::from_utf8_lossy(body).into_owned();
        let s = svc.session(doc)?;
        if s.busy.load(Ordering::Acquire) {
            return Err("busy".into());
        }
        let mut f = s.file.lock();
        if f.doc.read_only {
            return Err("read-only".into());
        }
        let out = f.doc.apply(&Action::Paste { text }).map_err(io_err)?;
        if out.changed {
            s.after_change(&mut f);
        }
        view_message(&s, &f, &req)
    })
}

fn write_clipboard(app: &AppHandle, text: String) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    if let Err(err) = app.clipboard().write_text(text) {
        tracing::warn!(%err, "clipboard write failed");
    }
}

#[tauri::command]
pub async fn txt_copy(
    app: AppHandle,
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
) -> CmdResult<bool> {
    let s = svc.session(doc)?;
    let text = s.file.lock().doc.copy().map_err(io_err)?;
    Ok(match text {
        Some(t) => {
            write_clipboard(&app, t);
            true
        }
        None => false,
    })
}

#[tauri::command]
pub async fn txt_seek(svc: State<'_, Arc<TxtService>>, doc: u64, fraction: f64) -> CmdResult<u64> {
    let s = svc.session(doc)?;
    let f = s.file.lock();
    view::seek(&f.doc, fraction).map_err(io_err)
}

/// Start of line `line` (0-based): exact once indexed, else estimated (§4:
/// approximate jumps before indexing finishes).
#[tauri::command]
pub async fn txt_goto_line(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    line: u64,
) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    let started = Instant::now();
    let mut f = s.file.lock();
    let (offset, exact) = f.doc.snapshot().line_start_estimate(line);
    f.doc
        .set_selections(vec![quire_text::document::Selection::caret(offset)], 0);
    Ok(json!({"offset": offset, "exact": exact, "ms": started.elapsed().as_secs_f64() * 1000.0}))
}

/// Starts a full background search; batches of matches stream over
/// `channel` as binary (u64 pairs, little-endian) with progress events on
/// the document channel.
#[tauri::command]
pub async fn txt_search(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    query: Query,
    channel: Channel<tauri::ipc::InvokeResponseBody>,
) -> CmdResult<u64> {
    let s = svc.session(doc)?;
    let matcher = Matcher::new(&query)?;
    let cancel = CancelToken::new();
    let (generation, snap, revision) = {
        let f = s.file.lock();
        let mut st = s.search.lock();
        if let Some(c) = st.cancel.take() {
            c.cancel();
        }
        st.generation += 1;
        st.matcher = Some(matcher.clone());
        st.matches.clear();
        st.progress = SearchProgress::default();
        st.revision = f.doc.revision();
        st.cancel = Some(cancel.clone());
        (st.generation, f.doc.snapshot(), f.doc.revision())
    };
    let scheduler = Arc::clone(&svc.scheduler);
    let session = Arc::clone(&s);
    std::thread::Builder::new()
        .name("quire-search".into())
        .spawn(move || {
            let started = Instant::now();
            let mut first_hit_ms = None;
            let result = search::search_all(
                &snap,
                &matcher,
                &scheduler,
                threads(),
                &cancel,
                &mut |batch, p| {
                    if !batch.is_empty() && first_hit_ms.is_none() {
                        first_hit_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                    }
                    {
                        let mut st = session.search.lock();
                        if st.generation != generation {
                            return;
                        }
                        st.matches.extend_from_slice(batch);
                        st.progress = *p;
                    }
                    if !batch.is_empty() {
                        let mut bytes = Vec::with_capacity(batch.len() * 16);
                        for (a, b) in batch {
                            bytes.extend_from_slice(&a.to_le_bytes());
                            bytes.extend_from_slice(&b.to_le_bytes());
                        }
                        let _ = channel.send(tauri::ipc::InvokeResponseBody::Raw(bytes));
                    }
                    session.emit(json!({
                        "kind": "search", "doc": session.id, "generation": generation,
                        "revision": revision, "progress": p, "firstHitMs": first_hit_ms,
                        "ms": started.elapsed().as_secs_f64() * 1000.0,
                    }));
                },
            );
            if let Err(err) = result {
                session.emit(
                    json!({"kind": "searchError", "doc": session.id, "error": err.to_string()}),
                );
            }
        })
        .map_err(io_err)?;
    Ok(generation)
}

#[tauri::command]
pub async fn txt_search_cancel(svc: State<'_, Arc<TxtService>>, doc: u64) -> CmdResult<()> {
    let s = svc.session(doc)?;
    let mut st = s.search.lock();
    if let Some(c) = st.cancel.take() {
        c.cancel();
    }
    st.matches.clear();
    st.matcher = None;
    Ok(())
}

/// Selects the next (or previous) match from the primary caret, from the
/// stored results when they're current, else by scanning.
#[tauri::command]
pub async fn txt_find(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    query: Query,
    backwards: bool,
    replace: Option<String>,
) -> CmdResult<Option<(u64, u64)>> {
    guarded(|| {
        let s = svc.session(doc)?;
        let mut f = s.file.lock();
        let m = Matcher::new(&query)?;
        let snap = f.doc.snapshot();
        let p = f.doc.primary();
        // Replace the current match first, if the selection is one.
        if let Some(r) = &replace {
            if s.busy.load(Ordering::Acquire) || f.doc.read_only {
                return Err("read-only or busy".into());
            }
            let sel = (p.start(), p.end());
            let hit = search::search_range(&snap, &m, sel.0, snap.len(), 1).map_err(io_err)?;
            if hit.first() == Some(&sel) && sel.0 < sel.1 {
                let text = search::expand(&snap, &m, sel, r).map_err(io_err)?;
                f.doc
                    .apply(&Action::Paste {
                        text: String::from_utf8_lossy(&text).into_owned(),
                    })
                    .map_err(io_err)?;
                s.after_change(&mut f);
            }
        }
        let snap = f.doc.snapshot();
        let p = f.doc.primary();
        let hit = if backwards {
            search::find_prev(&snap, &m, p.start())
        } else {
            search::find_next(&snap, &m, if p.is_empty() { p.head } else { p.end() })
        }
        .map_err(io_err)?;
        if let Some((a, b)) = hit {
            f.doc
                .set_selections(vec![quire_text::document::Selection::new(a, b)], 0);
        }
        Ok(hit)
    })
}

/// Replace-all as one background transaction with progress and cancel.
#[tauri::command]
pub async fn txt_replace_all(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    query: Query,
    replacement: String,
) -> CmdResult<()> {
    let s = svc.session(doc)?;
    let m = Matcher::new(&query)?;
    if s.busy.swap(true, Ordering::AcqRel) {
        return Err("busy".into());
    }
    let (snap, adder, temp) = {
        let f = s.file.lock();
        if f.doc.read_only {
            s.busy.store(false, Ordering::Release);
            return Err("read-only".into());
        }
        (
            f.doc.snapshot(),
            f.doc.buffer().adder(),
            f.doc.temp_dir.clone(),
        )
    };
    let cancel = CancelToken::new();
    if let Some(old) = s.search.lock().cancel.replace(cancel.clone()) {
        old.cancel();
    }
    let session = Arc::clone(&s);
    std::thread::Builder::new()
        .name("quire-replace-all".into())
        .spawn(move || {
            let started = Instant::now();
            let total = snap.len().max(1);
            let progress = |done: u64| {
                session.emit(json!({"kind": "replaceProgress", "doc": session.id,
                    "fraction": done as f64 / total as f64}));
            };
            let result = quire_text::replace::compute(&snap, &adder, &m, &replacement, &temp, &cancel, &progress);
            let msg = match result {
                Ok(Some(r)) => {
                    let mut f = session.file.lock();
                    f.doc.swap_tree(
                        r.tree,
                        quire_text::document::Record::ReplaceAll {
                            query: m.query.clone(),
                            replacement: replacement.clone(),
                        },
                    );
                    session.after_change(&mut f);
                    json!({"kind": "replaced", "doc": session.id, "count": r.count,
                        "materialized": r.materialized, "ms": started.elapsed().as_secs_f64() * 1000.0})
                }
                Ok(None) => json!({"kind": "replaced", "doc": session.id, "cancelled": true}),
                Err(err) => json!({"kind": "replaced", "doc": session.id, "error": err.to_string()}),
            };
            session.busy.store(false, Ordering::Release);
            session.emit(msg);
            let st = session.status(&session.file.lock());
            session.emit(st);
        })
        .map_err(io_err)?;
    Ok(())
}

#[tauri::command]
pub async fn txt_cancel(svc: State<'_, Arc<TxtService>>, doc: u64) -> CmdResult<()> {
    let s = svc.session(doc)?;
    if let Some(c) = s.search.lock().cancel.take() {
        c.cancel();
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "result")]
pub enum SaveOutcome {
    Saved {
        report: SaveReport,
    },
    /// Characters the encoding can't hold: ask, then save with `replace`.
    NeedsConsent {
        count: u64,
        sample: String,
    },
}

/// Saves (to `path` for Save As). The write runs from a snapshot while
/// edits are refused; only the replace step holds the document.
#[tauri::command]
pub async fn txt_save(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    path: Option<String>,
    replace_unmappable: bool,
) -> CmdResult<SaveOutcome> {
    let s = svc.session(doc)?;
    if s.busy.swap(true, Ordering::AcqRel) {
        return Err("busy".into());
    }
    let result = (|| {
        let target = path.map(PathBuf::from);
        let (snap, origin, format, current) = {
            let f = s.file.lock();
            (
                f.doc.snapshot(),
                f.origin.clone(),
                f.doc.format,
                f.path.clone(),
            )
        };
        let on = if replace_unmappable {
            OnUnmappable::Replace
        } else {
            OnUnmappable::Fail
        };
        s.emit(json!({"kind": "saving", "doc": s.id}));
        // Stop background readers of the old mapping first (§5.8 step 3).
        s.index_cancel.lock().cancel();
        if let Some(c) = s.search.lock().cancel.take() {
            c.cancel();
        }
        let prepared = match prepare_save(
            &snap,
            origin.as_ref(),
            format,
            current.as_deref(),
            target.as_deref(),
            on,
        ) {
            Ok(p) => p,
            Err(SaveError::Unmappable { count, sample }) => {
                return Ok(SaveOutcome::NeedsConsent { count, sample });
            }
            Err(e) => return Err(e.to_string()),
        };
        drop(snap);
        drop(origin);
        let temp = data_dir().join("tmp");
        let mut f = s.file.lock();
        let old_path = f.path.clone();
        let report = commit_save(
            &mut f,
            prepared,
            &SaveOptions {
                on_unmappable: on,
                threads: threads(),
                temp_dir: &temp,
            },
        )
        .map_err(|e| e.to_string())?;
        // The file now holds everything: start a fresh journal on the next edit.
        if let Some(w) = s.journal.lock().take() {
            w.close(true);
        }
        if old_path != f.path {
            if let Some(p) = &old_path {
                svc.unwatch(p);
            }
            if let Some(p) = &f.path {
                svc.watch(p);
            }
        }
        f.edit_lock = None;
        Ok(SaveOutcome::Saved { report })
    })();
    s.busy.store(false, Ordering::Release);
    // A failed save may have stopped the index of the (still current) original.
    let original = s.file.lock().original_index();
    svc.start_index(&s, original);
    let st = s.status(&s.file.lock());
    s.emit(st);
    result
}

#[tauri::command]
pub async fn txt_set_format(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    encoding: String,
    bom: bool,
    line_ending: Option<LineEnding>,
) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    let enc = encoding_rs::Encoding::for_label(encoding.as_bytes()).ok_or("unknown encoding")?;
    let mut f = s.file.lock();
    let bom = bom
        && (enc == encoding_rs::UTF_8
            || enc == encoding_rs::UTF_16LE
            || enc == encoding_rs::UTF_16BE);
    f.doc.set_format(SaveFormat {
        encoding: enc,
        bom,
        line_ending,
    });
    s.after_change(&mut f);
    Ok(s.status(&f))
}

/// Re-reads the file with another encoding (refused with unsaved edits).
#[tauri::command]
pub async fn txt_reopen(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    encoding: String,
) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    if s.file.lock().doc.is_dirty() {
        return Err("save or discard your changes before reopening with another encoding".into());
    }
    let enc = encoding_rs::Encoding::for_label(encoding.as_bytes()).ok_or("unknown encoding")?;
    svc.reload(&s, Some(enc))?;
    Ok(s.status(&s.file.lock()))
}

/// Allows editing a file that looked binary (the user confirmed).
#[tauri::command]
pub async fn txt_allow_edit(svc: State<'_, Arc<TxtService>>, doc: u64) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    let mut f = s.file.lock();
    f.doc.read_only = false;
    Ok(s.status(&f))
}

#[tauri::command]
pub async fn txt_status(svc: State<'_, Arc<TxtService>>, doc: u64) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    Ok(s.status(&s.file.lock()))
}

#[tauri::command]
pub async fn txt_check_disk(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
) -> CmdResult<Option<DiskStatus>> {
    let s = svc.session(doc)?;
    svc.check_disk(&s)
}

/// The user's answer to an external change: "reload", "keep" or "compare".
#[tauri::command]
pub async fn txt_disk_resolve(
    svc: State<'_, Arc<TxtService>>,
    doc: u64,
    choice: String,
) -> CmdResult<Value> {
    let s = svc.session(doc)?;
    match choice.as_str() {
        "reload" => {
            svc.reload(&s, None)?;
            Ok(json!({"reloaded": true}))
        }
        "keep" => {
            let mut f = s.file.lock();
            let now = watch::stamp_changed(&f).map_err(io_err)?;
            if let Some(now) = now {
                let status = watch::classify(&f, now).map_err(io_err)?;
                if !status.keep_mine_possible {
                    return Err("the bytes this document refers to were changed on disk".into());
                }
                watch::keep_mine(&mut f, &status);
            }
            Ok(json!({"kept": true}))
        }
        "compare" => {
            let f = s.file.lock();
            match watch::compare(&f).map_err(io_err)? {
                Some(h) => Ok(json!({"hunks": h})),
                None => Ok(json!({"tooLarge": true, "limit": watch::COMPARE_MAX})),
            }
        }
        _ => Err("unknown choice".into()),
    }
}

#[tauri::command]
pub async fn txt_recovery_list() -> CmdResult<Vec<journal::Orphan>> {
    Ok(journal::orphans(&data_dir().join("journal")))
}

#[tauri::command]
pub async fn txt_recover(
    svc: State<'_, Arc<TxtService>>,
    journal_path: String,
    channel: Channel<Value>,
) -> CmdResult<DocInfo> {
    guarded(|| {
        let started = Instant::now();
        let j = journal::read(Path::new(&journal_path)).map_err(io_err)?;
        let file = journal::recover(&j, &svc.governor, &data_dir().join("tmp"), threads())
            .map_err(io_err)?;
        let id = svc.next_id.fetch_add(1, Ordering::Relaxed);
        svc.adopt(id, file);
        let s = svc.session(id)?;
        *s.events.lock() = Some(channel);
        // Journal the recovered state anew, then drop the old journal.
        {
            let mut f = s.file.lock();
            let records: Vec<_> = j.records.iter().map(|r| r.0.clone()).collect();
            let path = journal::journal_name(f.path.as_deref());
            if let Ok(w) =
                JournalWriter::create(&data_dir().join("journal").join(path), &header(&f))
            {
                w.append(records);
                *s.journal.lock() = Some(w);
            }
            s.after_change(&mut f);
        }
        let _ = std::fs::remove_file(&journal_path);
        let status = s.status(&s.file.lock());
        Ok(DocInfo {
            doc: id,
            status,
            open_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    })
}

#[tauri::command]
pub async fn txt_recovery_discard(journal_path: String) -> CmdResult<()> {
    let p = PathBuf::from(&journal_path);
    if p.parent() != Some(&data_dir().join("journal")) {
        return Err("not a journal".into());
    }
    std::fs::remove_file(p).map_err(io_err)
}

/// The window is closing: close every document, keeping journals of those
/// with unsaved edits (the frontend already asked the user).
pub fn shutdown(svc: &TxtService) {
    let sessions: Vec<Arc<Session>> = svc.sessions.lock().values().cloned().collect();
    for s in sessions {
        let dirty = s.file.lock().doc.is_dirty();
        svc.close_session(&s, dirty);
    }
}
