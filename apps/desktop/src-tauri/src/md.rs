//! Markdown documents in the host (ADR-0030).
//!
//! A `.md` file is a TXT session (the text engine is the source of truth,
//! §6.2) with a Markdown state attached: the block index, the tables and the
//! preview's render cache (`quire-markdown`). The state follows the text
//! through the session's change listener, which only records the change
//! (folded into one envelope with any not yet applied) and wakes the
//! document's worker, so typing never waits for Markdown work.
//!
//! The worker applies changes, indexes the rest of the document in the
//! background one stretch at a time, and after each change pushes the
//! preview's current window to the frontend on the document's binary
//! Channel, with HTML only for blocks the frontend doesn't have. The
//! frontend asks for other windows with `md_view` (scrolling, jumps).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex, RwLock};
use quire_core::memory::{CacheClass, DocId, Evictable, MemoryGovernor, Registration};
use quire_highlight::service::{self as highlight_service, Listener};
use quire_markdown::document::MarkdownDoc;
use quire_markdown::document::render_document;
use quire_markdown::images::ImagePolicy;
use quire_markdown::preview::{self, Highlight, Preview, Window};
use quire_markdown::render::RenderOptions;
use quire_markdown::scan::BlockKind;
use quire_markdown::table_edit::{TableAction, edit_table};
use quire_markdown::words::WordCounter;
use quire_text::buffer::Snapshot;
use quire_text::document::TextChange;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody, Response};

use crate::images::{self, ImageService};
use crate::txt::{CmdResult, TxtService, guarded, io_err};

/// Whether a path is opened as Markdown.
pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "mkdn"
        )
    })
}

/// Rendered blocks kept per document before the oldest go (the governor can
/// ask for less).
const PREVIEW_CACHE: u64 = 64 * 1024 * 1024;
/// One highlighting pass over the shown code (ADR-0029): the state lock is
/// held this long at most, plus the rendering in progress.
const HIGHLIGHT_SLICE: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug)]
struct Viewport {
    offset: u64,
    before: usize,
    after: usize,
}

struct MdState {
    md: MarkdownDoc,
    preview: Preview,
    /// The text the state describes.
    snap: Snapshot,
    revision: u64,
    viewport: Option<Viewport>,
    /// Renderings the frontend has (block id → revision).
    sent: HashMap<u64, u64>,
    /// The last window pushed or answered: (id, rev) in order.
    shown: Vec<(u64, u64)>,
    index_started: Instant,
    index_ms: Option<f64>,
}

struct Pending {
    change: TextChange,
    snap: Snapshot,
    revision: u64,
    at: Instant,
}

pub struct MdDoc {
    doc: u64,
    state: Mutex<MdState>,
    pending: Mutex<Option<Pending>>,
    wake: Condvar,
    channel: Mutex<Option<Channel<InvokeResponseBody>>>,
    events: Mutex<Option<Channel<Value>>>,
    closed: AtomicBool,
    cache_bytes: Mutex<Option<Registration>>,
    index_bytes: Registration,
    /// Last edit applied: (text revision, ms from the change to the push).
    last_edit: Mutex<Option<(u64, f64)>>,
    /// When the word count is due (after the index completes, and a moment
    /// after the last edit).
    words_due: Mutex<Option<Instant>>,
    /// The session's readers gate (`txt::Session::readers`): held shared
    /// while reading the text, so a save never re-points it under a read.
    readers: Arc<RwLock<()>>,
    /// Counts snapshot replacements by saves (a reader holding a clone
    /// of the snapshot starts over when it changes).
    snap_gen: AtomicU64,
    /// Wakes the worker when background highlighting is ready (ADR-0029);
    /// registered weakly with `quire_highlight::service`.
    highlight_listener: Arc<Listener>,
}

impl Evictable for MdDoc {
    fn evict(&self, bytes: u64) -> u64 {
        let Some(mut st) = self.state.try_lock() else {
            return 0;
        };
        let target = st.preview.bytes().saturating_sub(bytes);
        let freed = st.preview.trim(target);
        drop(st);
        self.charge();
        freed
    }
}

impl MdDoc {
    fn charge(&self) {
        let bytes = self.state.try_lock().map(|s| s.preview.bytes());
        if let (Some(b), Some(reg)) = (bytes, self.cache_bytes.lock().as_ref()) {
            reg.charge(b as i64 - reg.bytes() as i64);
        }
    }

    fn push(&self, bytes: Vec<u8>) {
        if let Some(c) = self.channel.lock().as_ref()
            && let Err(err) = c.send(InvokeResponseBody::Raw(bytes))
        {
            tracing::debug!(%err, "preview window dropped");
        }
    }

    fn emit(&self, v: Value) {
        if let Some(c) = self.events.lock().as_ref() {
            let _ = c.send(v);
        }
    }

    /// Records a change (listener, under the text lock: only cheap work).
    ///
    /// An empty change is a save: the text is the same, but the old
    /// snapshot now reads the saved file through its old pieces. The save
    /// holds the readers gate exclusively, so no reader is inside the state:
    /// the snapshot is replaced right here, before anything reads it again.
    fn record(&self, change: TextChange, snap: Snapshot, revision: u64) {
        if is_empty(change) {
            // State, then pending (the order apply_pending takes them in).
            let mut st = self.state.lock();
            self.snap_gen.fetch_add(1, Ordering::AcqRel);
            match self.pending.lock().as_mut() {
                Some(prev) => {
                    prev.snap = snap;
                    prev.revision = revision;
                }
                None => {
                    st.snap = snap;
                    st.revision = revision;
                }
            }
            return;
        }
        let mut p = self.pending.lock();
        *p = Some(match p.take() {
            Some(prev) => Pending {
                change: prev.change.then(change),
                snap,
                revision,
                at: prev.at,
            },
            None => Pending {
                change,
                snap,
                revision,
                at: Instant::now(),
            },
        });
        self.wake.notify_one();
    }

    /// Applies a recorded change to the state; returns when it was made.
    fn apply_pending(&self, st: &mut MdState) -> CmdResult<Option<Instant>> {
        let Some(p) = self.pending.lock().take() else {
            return Ok(None);
        };
        let c = p.change;
        if is_empty(c) {
            st.snap = p.snap;
            st.revision = p.revision;
            return Ok(None);
        }
        let ch = st
            .md
            .edit(&p.snap, c.from, c.removed, c.inserted)
            .map_err(io_err)?;
        st.preview.forget(&ch.removed);
        for id in &ch.removed {
            st.sent.remove(id);
        }
        st.snap = p.snap;
        st.revision = p.revision;
        if let Some(v) = st.viewport.as_mut() {
            let old_end = c.from + c.removed;
            if v.offset >= old_end {
                v.offset = v.offset + c.inserted - c.removed;
            } else if v.offset > c.from {
                v.offset = c.from;
            }
        }
        Ok(Some(p.at))
    }

    /// Applies a recorded change and pushes the window as it is now.
    ///
    /// Whoever takes the change pushes it: the worker, or a request that
    /// reads the state first (`md_view`, `md_blocks`). Otherwise an edit a
    /// request took would first show with the corrections, after the
    /// O(document) rebuild of the tables.
    fn apply_edit(&self, st: &mut MdState) -> CmdResult<()> {
        let Some(at) = self.apply_pending(st)? else {
            return Ok(());
        };
        // First the window as it is now, against the tables as they are:
        // the edited block's text and structure don't wait for an
        // O(document) rebuild of the tables. If the edit changed what a
        // block contributes (a footnote reference, a definition, a heading),
        // the corrections follow once no further edit is waiting.
        st.preview.set_provisional(true);
        // The edited block keeps its highlighting (ADR-0029).
        st.preview.set_highlight(Highlight::New);
        let pushed = self.push_window(st, false);
        st.preview.set_highlight(Highlight::Off);
        let indexing = !st.md.index().is_complete();
        st.preview.set_provisional(indexing);
        let pushed = pushed?;
        *self.words_due.lock() = Some(Instant::now() + WORDS_DELAY);
        let ms = at.elapsed().as_secs_f64() * 1000.0;
        *self.last_edit.lock() = Some((st.revision, ms));
        if pushed {
            tracing::trace!(ms, "preview patch pushed");
        }
        Ok(())
    }

    /// The window around `v`, marking what it sends as sent.
    fn window(&self, st: &mut MdState, v: Viewport) -> CmdResult<Window> {
        let MdState {
            md,
            preview,
            snap,
            sent,
            ..
        } = st;
        let w = preview
            .window(md, &*snap, v.offset, v.before, v.after, &|id, rev| {
                sent.get(&id) == Some(&rev)
            })
            .map_err(io_err)?;
        for b in &w.blocks {
            st.sent.insert(b.id, b.rev);
        }
        st.shown = w.blocks.iter().map(|b| (b.id, b.rev)).collect();
        if st.preview.bytes() > PREVIEW_CACHE {
            st.preview.trim(PREVIEW_CACHE * 3 / 4);
        }
        Ok(w)
    }

    /// Pushes the current window if it differs from the one shown.
    fn push_window(&self, st: &mut MdState, force: bool) -> CmdResult<bool> {
        let Some(v) = st.viewport else {
            return Ok(false);
        };
        let before = std::mem::take(&mut st.shown);
        let w = self.window(st, v)?;
        let changed = force || st.shown != before || w.blocks.iter().any(|b| b.html.is_some());
        if changed {
            self.push(preview::encode(&w, 0, st.revision));
        }
        Ok(changed)
    }

    fn run(self: Arc<Self>) {
        loop {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            let has_pending = self.pending.lock().is_some();
            let (complete, tables_pending) = {
                let st = self.state.lock();
                (st.md.index().is_complete(), st.md.tables_pending())
            };
            // Tables wait for a rebuild only after the index is complete
            // (until then every stretch changes them; the completion push
            // brings them).
            let corrections = complete && tables_pending;
            let highlight = !has_pending && !corrections && {
                let st = self.state.lock();
                st.preview.highlight_due(&st.shown)
            };
            if !has_pending && complete && !corrections && !highlight {
                let due = *self.words_due.lock();
                if due.is_some_and(|d| d <= Instant::now()) {
                    self.count_words();
                    continue;
                }
                let wait = due.map_or(Duration::from_millis(500), |d| {
                    d.saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(500))
                });
                let mut p = self.pending.lock();
                if p.is_none() && !self.closed.load(Ordering::Acquire) {
                    self.wake.wait_for(&mut p, wait);
                }
                continue;
            }
            let _reading = self.readers.read();
            let mut st = self.state.lock();
            let step = if has_pending {
                self.apply_edit(&mut st)
            } else if highlight {
                // Shown code, highlighted a slice at a time (ADR-0029).
                st.preview
                    .set_highlight(Highlight::Upgrade(HIGHLIGHT_SLICE));
                let r = self.push_window(&mut st, false).map(|_| ());
                st.preview.set_highlight(Highlight::Off);
                r
            } else if corrections {
                let started = Instant::now();
                let r = self.push_window(&mut st, false).map(|_| ());
                tracing::debug!(
                    ms = started.elapsed().as_secs_f64() * 1000.0,
                    "tables rebuilt after an edit"
                );
                r
            } else {
                let MdState { md, snap, .. } = &mut *st;
                match md.extend(&*snap) {
                    Ok(_) if md.index().is_complete() => {
                        let ms = st.index_started.elapsed().as_secs_f64() * 1000.0;
                        st.index_ms = Some(ms);
                        tracing::info!(
                            doc = self.doc,
                            ms,
                            blocks = st.md.index().blocks().len(),
                            "markdown index complete"
                        );
                        self.index_bytes.charge(
                            index_estimate(&st.md) as i64 - self.index_bytes.bytes() as i64,
                        );
                        *self.words_due.lock() = Some(Instant::now());
                        // Definitions and footnotes from the whole document.
                        st.preview.set_provisional(false);
                        let r = self.push_window(&mut st, false);
                        let outline = st.md.tables().outline().len();
                        self.emit(json!({"kind": "indexed", "ms": ms, "blocks": st.md.index().blocks().len(), "headings": outline}));
                        r.map(|_| ())
                    }
                    Ok(_) => Ok(()),
                    Err(e) => Err(io_err(e)),
                }
            };
            drop(st);
            self.charge();
            if let Err(e) = step {
                tracing::error!(doc = self.doc, %e, "markdown worker step failed");
                // Leave it to the next change or request.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    /// Counts the words of the current text, off the state lock; gives up
    /// (to count again later) when the text changes meanwhile.
    fn count_words(&self) {
        let (snap, revision, generation) = {
            let st = self.state.lock();
            (
                st.snap.clone(),
                st.revision,
                self.snap_gen.load(Ordering::Acquire),
            )
        };
        *self.words_due.lock() = None;
        let started = Instant::now();
        let mut counter = WordCounter::new();
        let len = snap.len();
        let mut at = 0;
        while at < len {
            if self.pending.lock().is_some() || self.closed.load(Ordering::Acquire) {
                *self.words_due.lock() = Some(Instant::now() + WORDS_DELAY);
                return;
            }
            let end = (at + WORDS_CHUNK).min(len);
            let _reading = self.readers.read();
            if self.snap_gen.load(Ordering::Acquire) != generation {
                *self.words_due.lock() = Some(Instant::now());
                return;
            }
            match snap.read(at..end) {
                Ok(bytes) => counter.push(&bytes),
                Err(e) => {
                    tracing::warn!(%e, "word count stopped");
                    return;
                }
            }
            at = end;
        }
        let words = counter.finish();
        tracing::debug!(
            doc = self.doc,
            words,
            ms = started.elapsed().as_secs_f64() * 1000.0,
            "words counted"
        );
        self.emit(json!({"kind": "words", "words": words, "revision": revision}));
    }
}

/// A change that changes no text (a save: only the snapshot is new).
fn is_empty(c: TextChange) -> bool {
    c.removed == 0 && c.inserted == 0
}

/// Wait after an edit before counting words again.
const WORDS_DELAY: Duration = Duration::from_millis(400);
const WORDS_CHUNK: u64 = 1 << 20;

impl MdDoc {
    /// Kinds and ranges of the blocks overlapping `from..to`, for the source
    /// view's decoration: per block u64 start, u64 end, u8 kind, u8 detail.
    fn blocks_in(&self, from: u64, to: u64) -> CmdResult<Vec<u8>> {
        let _reading = self.readers.read();
        let mut st = self.state.lock();
        self.apply_edit(&mut st)?;
        let MdState {
            md, snap, revision, ..
        } = &mut *st;
        md.extend_to(&*snap, to).map_err(io_err)?;
        let blocks = md.index().blocks();
        let first = md.index().block_at(from).min(blocks.len());
        let mut out = Vec::new();
        out.extend_from_slice(&revision.to_le_bytes());
        for b in blocks[first..]
            .iter()
            .take_while(|b| b.start < to)
            .take(4096)
        {
            out.extend_from_slice(&b.start.to_le_bytes());
            out.extend_from_slice(&b.end.to_le_bytes());
            out.push(preview::kind_code(&b.kind));
            out.push(preview::kind_detail(&b.kind));
        }
        Ok(out)
    }
}

/// Rough bytes the index and analyses take (counted against the document).
fn index_estimate(md: &MarkdownDoc) -> u64 {
    md.index().blocks().len() as u64 * 96
}

pub struct MdService {
    docs: Mutex<HashMap<u64, Arc<MdDoc>>>,
    txt: Arc<TxtService>,
    images: Arc<ImageService>,
    governor: MemoryGovernor,
}

impl MdService {
    pub fn new(txt: Arc<TxtService>, images: Arc<ImageService>, governor: MemoryGovernor) -> Self {
        Self {
            docs: Mutex::new(HashMap::new()),
            txt,
            images,
            governor,
        }
    }

    fn get(&self, doc: u64) -> CmdResult<Arc<MdDoc>> {
        self.docs
            .lock()
            .get(&doc)
            .cloned()
            .ok_or_else(|| format!("no Markdown state for document {doc}"))
    }

    fn options(path: Option<&Path>, remote: Option<u64>) -> RenderOptions {
        RenderOptions {
            sanitize: Some(ImagePolicy {
                base_dir: path.and_then(Path::parent).map(Path::to_path_buf),
                scheme: images::url_prefix().to_owned(),
                remote,
            }),
            ..RenderOptions::default()
        }
    }

    /// Attaches Markdown state to TXT document `doc`.
    pub fn attach(
        &self,
        doc: u64,
        channel: Channel<InvokeResponseBody>,
        events: Channel<Value>,
    ) -> CmdResult<()> {
        if let Some(old) = self.docs.lock().remove(&doc) {
            old.closed.store(true, Ordering::Release);
            old.wake.notify_all();
        }
        let s = self.txt.session(doc)?;
        let mut f = s.file.lock();
        let snap = f.doc.snapshot();
        let revision = f.doc.revision();
        let opts = Self::options(f.path.as_deref(), None);
        let md = Arc::new_cyclic(|weak: &Weak<MdDoc>| {
            let me = weak.clone();
            let highlight_listener: Arc<Listener> = Arc::new(move || {
                if let Some(m) = me.upgrade() {
                    let _p = m.pending.lock();
                    m.wake.notify_one();
                }
            });
            let weak: Weak<dyn Evictable> = weak.clone();
            let reg =
                self.governor
                    .register("md-preview", Some(DocId(doc)), CacheClass::Layout, weak);
            MdDoc {
                doc,
                state: Mutex::new(MdState {
                    md: MarkdownDoc::new(snap.len()),
                    preview: {
                        let mut p = Preview::new(opts);
                        // Until the index completes, render with what is
                        // known (no rebuild of partial tables per request).
                        p.set_provisional(true);
                        p
                    },
                    snap: snap.clone(),
                    revision,
                    viewport: None,
                    sent: HashMap::new(),
                    shown: Vec::new(),
                    index_started: Instant::now(),
                    index_ms: None,
                }),
                pending: Mutex::new(None),
                wake: Condvar::new(),
                channel: Mutex::new(Some(channel)),
                events: Mutex::new(Some(events)),
                closed: AtomicBool::new(false),
                cache_bytes: Mutex::new(Some(reg)),
                index_bytes: self.governor.register_fixed("md-index", Some(DocId(doc))),
                last_edit: Mutex::new(None),
                words_due: Mutex::new(None),
                readers: Arc::clone(&s.readers),
                snap_gen: AtomicU64::new(0),
                highlight_listener,
            }
        });
        highlight_service::add_listener(Arc::downgrade(&md.highlight_listener));
        let weak = Arc::downgrade(&md);
        s.set_listener(
            &mut f,
            Some(Arc::new(move |change, file| {
                if let Some(m) = weak.upgrade() {
                    m.record(change, file.doc.snapshot(), file.doc.revision());
                }
            })),
        );
        drop(f);
        self.docs.lock().insert(doc, Arc::clone(&md));
        let worker = Arc::clone(&md);
        std::thread::Builder::new()
            .name(format!("quire-md-{doc}"))
            .spawn(move || worker.run())
            .map_err(io_err)?;
        Ok(())
    }

    pub fn detach(&self, doc: u64) {
        if let Some(m) = self.docs.lock().remove(&doc) {
            m.closed.store(true, Ordering::Release);
            m.wake.notify_all();
            *m.channel.lock() = None;
            *m.events.lock() = None;
            *m.cache_bytes.lock() = None;
            if let Ok(s) = self.txt.session(doc) {
                let mut f = s.file.lock();
                s.set_listener(&mut f, None);
            }
        }
        self.images.set_remote(doc, false);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attached {
    /// The image scheme's URL prefix (for the frontend's checks).
    pub image_prefix: &'static str,
    /// Where the Mermaid frame page is served (ADR-0033).
    pub frame_url: String,
}

#[tauri::command]
pub async fn md_attach(
    svc: State<'_, MdService>,
    doc: u64,
    channel: Channel<InvokeResponseBody>,
    events: Channel<Value>,
) -> CmdResult<Attached> {
    guarded(|| {
        svc.attach(doc, channel, events)?;
        Ok(Attached {
            image_prefix: images::url_prefix(),
            frame_url: format!("{}{}", crate::frame::url_prefix(), crate::frame::PAGE),
        })
    })
}

#[tauri::command]
pub async fn md_detach(svc: State<'_, MdService>, doc: u64) -> CmdResult<()> {
    svc.detach(doc);
    Ok(())
}

/// The preview window around `offset`. `forget` lists blocks the frontend
/// dropped (they come with HTML again).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn md_view(
    svc: State<'_, MdService>,
    doc: u64,
    seq: u64,
    offset: u64,
    before: u32,
    after: u32,
    forget: Option<Vec<u64>>,
) -> CmdResult<Response> {
    guarded(|| {
        let m = svc.get(doc)?;
        let _reading = m.readers.read();
        let mut st = m.state.lock();
        // Show the latest text.
        m.apply_edit(&mut st)?;
        for id in forget.unwrap_or_default() {
            st.sent.remove(&id);
        }
        let v = Viewport {
            offset: offset.min(st.snap.len()),
            before: before.min(400) as usize,
            after: after.min(400) as usize,
        };
        st.viewport = Some(v);
        let w = m.window(&mut st, v)?;
        let msg = preview::encode(&w, seq, st.revision);
        let highlight = st.preview.highlight_due(&st.shown);
        drop(st);
        if highlight {
            // The worker highlights the window's code next.
            let _p = m.pending.lock();
            m.wake.notify_one();
        }
        m.charge();
        Ok(Response::new(msg))
    })
}

/// Turns the document's "load remote content" on or off; the preview
/// renders again.
#[tauri::command]
pub async fn md_set_remote(svc: State<'_, MdService>, doc: u64, on: bool) -> CmdResult<()> {
    guarded(|| {
        let m = svc.get(doc)?;
        svc.images.set_remote(doc, on);
        let path = svc.txt.session(doc)?.file.lock().path.clone();
        let _reading = m.readers.read();
        let mut st = m.state.lock();
        st.preview
            .set_options(MdService::options(path.as_deref(), on.then_some(doc)));
        m.push_window(&mut st, true)?;
        Ok(())
    })
}

/// Tables larger than this aren't edited by the helpers.
const TABLE_EDIT_MAX: u64 = 1024 * 1024;

/// A table helper (task 10) at `offset`: the replacement and the caret, in
/// document offsets, for the text at `revision`; `null` when the offset
/// isn't in a table or the action doesn't apply. The frontend applies it
/// as one edit (`replace`) if the text is still at that revision.
#[tauri::command]
pub async fn md_table(
    svc: State<'_, MdService>,
    doc: u64,
    offset: u64,
    action: String,
) -> CmdResult<Value> {
    guarded(|| {
        let Some(action) = TableAction::parse(&action) else {
            return Err(format!("unknown table action {action}"));
        };
        let m = svc.get(doc)?;
        let _reading = m.readers.read();
        let mut st = m.state.lock();
        m.apply_edit(&mut st)?;
        let MdState {
            md, snap, revision, ..
        } = &mut *st;
        md.extend_to(&*snap, offset).map_err(io_err)?;
        let blocks = md.index().blocks();
        let Some(b) = blocks.get(md.index().block_at(offset)) else {
            return Ok(Value::Null);
        };
        if b.kind != BlockKind::Table || b.end - b.start > TABLE_EDIT_MAX || offset < b.start {
            return Ok(Value::Null);
        }
        let bytes = snap.read(b.range()).map_err(io_err)?;
        let Ok(src) = std::str::from_utf8(&bytes) else {
            return Ok(Value::Null);
        };
        let Some(e) = edit_table(src, (offset - b.start) as usize, action) else {
            return Ok(Value::Null);
        };
        Ok(json!({
            "revision": *revision,
            "from": b.start + e.from as u64,
            "to": b.start + e.to as u64,
            "text": e.text,
            "caret": b.start + e.caret as u64,
        }))
    })
}

/// A pasted image (task 10, decision A): the raw request body is the
/// image; it is stored as PNG in `assets/` next to the document. Returns
/// the relative link to insert.
#[tauri::command]
pub async fn md_paste_image(
    svc: State<'_, MdService>,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<String> {
    guarded(|| {
        let tauri::ipc::InvokeBody::Raw(body) = request.body() else {
            return Err("expected a raw body".into());
        };
        let doc: u64 = request
            .headers()
            .get("x-quire-doc")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or("missing header x-quire-doc")?;
        let path = svc.txt.session(doc)?.file.lock().path.clone();
        let Some(dir) = path.as_deref().and_then(Path::parent) else {
            return Err("save the document first: pasted images go next to it".into());
        };
        let png = crate::paste::to_png(body).map_err(io_err)?;
        crate::paste::store(dir, &png, &crate::paste::today()).map_err(io_err)
    })
}

/// Documents larger than this aren't exported.
const EXPORT_MAX: u64 = 256 * 1024 * 1024;

/// The whole document rendered for export (task 11, ADR-0031): HTML with
/// code highlighted and images inlined, the report of what was left out,
/// and a title (the file's name).
#[tauri::command]
pub async fn md_export_html(svc: State<'_, MdService>, doc: u64) -> CmdResult<Value> {
    guarded(|| {
        let s = svc.txt.session(doc)?;
        let (snap, path) = {
            let f = s.file.lock();
            (f.doc.snapshot(), f.path.clone())
        };
        if snap.len() > EXPORT_MAX {
            return Err("the document is too large to export (over 256 MiB)".into());
        }
        let bytes = {
            let _reading = s.readers.read();
            snap.read(0..snap.len()).map_err(io_err)?
        };
        let remote = svc.images.remote_enabled(doc).then_some(doc);
        let opts = RenderOptions {
            sourcepos: false,
            highlight: true,
            ..MdService::options(path.as_deref(), remote)
        };
        let body = render_document(&bytes, &opts);
        let mut report = Vec::new();
        let html =
            crate::export::inline_images(&body, images::url_prefix(), &svc.images, &mut report);
        let title = path.as_deref().and_then(Path::file_stem).map_or_else(
            || "Untitled".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        Ok(json!({ "html": html, "report": report, "title": title }))
    })
}

/// Writes an export: the raw body is the finished HTML page; header
/// `x-quire-path` (percent-encoded) is where, `x-quire-kind` is `html` or
/// `pdf` (printed by WebView2 from the page).
#[tauri::command]
pub async fn md_export_save(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<()> {
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
    let path = quire_markdown::images::percent_decode(&header("x-quire-path")?)
        .map(std::path::PathBuf::from)
        .ok_or("bad path")?;
    match header("x-quire-kind")?.as_str() {
        "html" => crate::export::write_file(&path, body).map_err(io_err),
        #[cfg(windows)]
        "pdf" => {
            let html = body.clone();
            tauri::async_runtime::spawn_blocking(move || {
                crate::export::print_pdf(&app, &html, &path)
            })
            .await
            .map_err(io_err)?
        }
        other => Err(format!("unknown export kind {other}")),
    }
}

/// Longest heading text sent to the outline rail.
const OUTLINE_TEXT_MAX: usize = 200;

/// The outline (task 9): headings in document order as
/// `[start byte of their block, level, text]`, with the tables' generation.
/// `null` while the index runs or the tables wait for a rebuild after an
/// edit, and when the frontend already has generation `since`.
#[tauri::command]
pub async fn md_outline(
    svc: State<'_, MdService>,
    doc: u64,
    since: Option<u64>,
) -> CmdResult<Value> {
    guarded(|| {
        let m = svc.get(doc)?;
        let st = m.state.lock();
        if !st.md.index().is_complete() || st.md.tables_pending() {
            return Ok(Value::Null);
        }
        let generation = st.md.current_generation();
        if since == Some(generation) {
            return Ok(Value::Null);
        }
        let starts: HashMap<u64, u64> = st
            .md
            .index()
            .blocks()
            .iter()
            .map(|b| (b.id, b.start))
            .collect();
        let entries: Vec<Value> = st
            .md
            .current_tables()
            .outline()
            .iter()
            .filter_map(|e| {
                let start = *starts.get(&e.block)?;
                let mut text = e.text.as_str();
                if text.len() > OUTLINE_TEXT_MAX {
                    let mut cut = OUTLINE_TEXT_MAX;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text = &text[..cut];
                }
                Some(json!([start, e.level, text]))
            })
            .collect();
        Ok(json!({ "generation": generation, "entries": entries }))
    })
}

/// Index state and the last edit's timing (status bar, benchmarks).
#[tauri::command]
pub async fn md_status(svc: State<'_, MdService>, doc: u64) -> CmdResult<Value> {
    let m = svc.get(doc)?;
    let st = m.state.lock();
    let last = *m.last_edit.lock();
    Ok(json!({
        "blocks": st.md.index().blocks().len(),
        "complete": st.md.index().is_complete(),
        "frontier": st.md.index().frontier(),
        "len": st.snap.len(),
        "revision": st.revision,
        "indexMs": st.index_ms,
        "cacheBytes": st.preview.bytes(),
        "lastEdit": last.map(|(rev, ms)| json!({"revision": rev, "ms": ms})),
        "remote": svc.images.remote_enabled(doc),
    }))
}

/// Block kinds over a byte range (source view decoration), binary:
/// u64 revision, then per block u64 start, u64 end, u8 kind, u8 detail.
#[tauri::command]
pub async fn md_blocks(
    svc: State<'_, MdService>,
    doc: u64,
    from: u64,
    to: u64,
) -> CmdResult<Response> {
    guarded(|| {
        let m = svc.get(doc)?;
        Ok(Response::new(m.blocks_in(from, to)?))
    })
}
