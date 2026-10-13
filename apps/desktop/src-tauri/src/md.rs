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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use quire_core::memory::{CacheClass, DocId, Evictable, MemoryGovernor, Registration};
use quire_markdown::document::MarkdownDoc;
use quire_markdown::images::ImagePolicy;
use quire_markdown::preview::{self, Preview, Window};
use quire_markdown::render::RenderOptions;
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
    fn record(&self, change: TextChange, snap: Snapshot, revision: u64) {
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
            let complete = self.state.lock().md.index().is_complete();
            if !has_pending && complete {
                let mut p = self.pending.lock();
                if p.is_none() && !self.closed.load(Ordering::Acquire) {
                    self.wake.wait_for(&mut p, Duration::from_millis(500));
                }
                continue;
            }
            let mut st = self.state.lock();
            let step = if has_pending {
                self.apply_pending(&mut st).and_then(|at| {
                    let pushed = self.push_window(&mut st, false)?;
                    if let Some(at) = at {
                        let ms = at.elapsed().as_secs_f64() * 1000.0;
                        *self.last_edit.lock() = Some((st.revision, ms));
                        if pushed {
                            tracing::trace!(ms, "preview patch pushed");
                        }
                    }
                    Ok(())
                })
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
                        // Definitions and footnotes from the whole document.
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
            let weak: Weak<dyn Evictable> = weak.clone();
            let reg =
                self.governor
                    .register("md-preview", Some(DocId(doc)), CacheClass::Layout, weak);
            MdDoc {
                doc,
                state: Mutex::new(MdState {
                    md: MarkdownDoc::new(snap.len()),
                    preview: Preview::new(opts),
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
            }
        });
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
        let mut st = m.state.lock();
        // Show the latest text.
        m.apply_pending(&mut st)?;
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
        drop(st);
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
        let mut st = m.state.lock();
        st.preview
            .set_options(MdService::options(path.as_deref(), on.then_some(doc)));
        m.push_window(&mut st, true)?;
        Ok(())
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
