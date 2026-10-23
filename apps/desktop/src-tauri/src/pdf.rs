//! PDF documents (M3, MASTER_PROMPT §6.6): opening through the PDFium
//! worker pool, the page index, and page tiles (ADR-0036–0038).
//!
//! The page index starts with what PDFium knows at open (the page count and
//! the first page's size); a background walk then asks for every page's
//! size in batches and streams them to the frontend, which lays the pages
//! out and labels the total height an estimate until the walk is done
//! (§7). The walk also makes PDFium load every page dictionary, which is
//! what makes later jumps fast (M3 plan: engine spike).
//!
//! Tiles are page-local (`quire_pdf::tiles`): the frontend asks for
//! (page, col, row) at a scale and rotation; each request bumps the
//! document's generation, so tiles no longer wanted are dropped before
//! they reach a worker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use quire_core::memory::{MemoryGovernor, Registration};
use quire_core::scheduler::{CancelToken, Generation, JobKey, JobSpec, Priority, Scheduler};
use quire_core::tile::{RenderedTile, TileKey};
use quire_pdf::tiles::{self, Rotation};
use quire_pdf::{PdfError, Pool, PoolConfig, WorkerCommand, protocol};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};

use crate::transport::TileSink;
use crate::txt::{CmdResult, guarded};
use crate::view::TilePriority;

/// Pages per page-size batch of the background walk.
const WALK_BATCH: u32 = 250;
/// See `view.rs`: re-requests of a just-delivered tile are ignored.
const IN_FLIGHT_TTL: Duration = Duration::from_millis(400);

/// Where the PDFium library is: `QUIRE_PDFIUM`, next to the executable
/// (installed builds), or the fetched copy in the repository's `.cache`
/// (development builds; `tools/pdfium/fetch-pdfium.ps1`).
pub fn find_library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QUIRE_PDFIUM") {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let beside = exe_dir.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        _ => "linux-x64",
    };
    let sub = if cfg!(windows) { "bin" } else { "lib" };
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.cache/pdfium/151.0.7881.0")
        .join(platform)
        .join(sub)
        .join(name);
    cache.is_file().then_some(cache)
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PageTileRequest {
    pub page: u32,
    pub col: u32,
    pub row: u32,
    pub priority: TilePriority,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfOpened {
    pub doc: u64,
    pub pages: u32,
    /// False when PDFium rebuilt a broken cross-reference table: the UI
    /// shows the "repaired" banner (§6.6).
    pub valid_xref: bool,
    pub version: u32,
    /// The first page's size in points; the others are estimated from it
    /// until the walk reports them.
    pub first_size: (f32, f32),
    pub transport: &'static str,
    pub open_ms: f64,
}

struct PdfDoc {
    pool_doc: u64,
    pages: u32,
    sizes: Mutex<Vec<Option<(f32, f32)>>>,
    generation: Generation,
    sink: Arc<dyn TileSink>,
    events: Channel<Value>,
    walk: CancelToken,
    delivered: Mutex<HashMap<TileKey, Instant>>,
    revision: u32,
}

pub struct PdfService {
    scheduler: Arc<Scheduler>,
    pool: OnceLock<Result<Pool, String>>,
    docs: Mutex<HashMap<u64, Arc<PdfDoc>>>,
    next: AtomicU64,
    workers_memory: Mutex<(Registration, u64)>,
    /// A PDF given on the command line, for the frontend to open.
    startup: Mutex<Option<String>>,
}

impl PdfService {
    pub fn new(
        scheduler: Arc<Scheduler>,
        governor: &MemoryGovernor,
        startup: Option<PathBuf>,
    ) -> Self {
        Self {
            startup: Mutex::new(startup.map(|p| p.to_string_lossy().into_owned())),
            scheduler,
            pool: OnceLock::new(),
            docs: Mutex::new(HashMap::new()),
            // Above the view service's ids: tiles of both share one receiver map.
            next: AtomicU64::new(1 << 40),
            workers_memory: Mutex::new((governor.register_fixed("pdf-workers", None), 0)),
        }
    }

    /// The pool, created on first use: workers run as this executable with
    /// `--pdf-worker` (ADR-0036).
    fn pool(&self) -> CmdResult<&Pool> {
        self.pool
            .get_or_init(|| {
                let library = find_library().ok_or_else(|| {
                    "The PDF engine (PDFium) isn't installed with this build.".to_owned()
                })?;
                let program = std::env::current_exe().map_err(|e| e.to_string())?;
                Ok(Pool::new(PoolConfig {
                    workers: quire_pdf::cpu::worker_count(),
                    command: WorkerCommand {
                        program,
                        args: vec!["--pdf-worker".into()],
                        library,
                        faults: std::env::var_os("QUIRE_PDF_FAULTS").is_some_and(|v| v == "1"),
                    },
                    timeout: Duration::from_secs(20),
                }))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn doc(&self, doc: u64) -> CmdResult<Arc<PdfDoc>> {
        self.docs
            .lock()
            .get(&doc)
            .cloned()
            .ok_or_else(|| format!("no PDF document {doc}"))
    }

    /// Charges the workers' private bytes to the governor (§5.5: they count
    /// against the per-document budget like any cache).
    fn account_workers(&self, pool: &Pool) {
        let bytes = pool.stats().private_bytes;
        let mut m = self.workers_memory.lock();
        let delta = bytes as i64 - m.1 as i64;
        if delta != 0 {
            m.0.charge(delta);
            m.1 = bytes;
        }
    }

    fn open(
        &self,
        path: &str,
        password: Option<&str>,
        sink: Arc<dyn TileSink>,
        events: Channel<Value>,
    ) -> CmdResult<PdfOpened> {
        let started = Instant::now();
        let pool = self.pool()?;
        let (pool_doc, info) = pool.open_document(path, password).map_err(open_error)?;
        let first_size = if info.pages > 0 {
            pool.page_sizes(pool_doc, 0, 1).map_err(open_error)?[0]
        } else {
            (612.0, 792.0)
        };
        let doc = self.next.fetch_add(1, Ordering::Relaxed);
        let mut sizes = vec![None; info.pages as usize];
        if let Some(s) = sizes.first_mut() {
            *s = Some(first_size);
        }
        let transport = sink.name();
        let d = Arc::new(PdfDoc {
            pool_doc,
            pages: info.pages,
            sizes: Mutex::new(sizes),
            generation: Generation::new(),
            sink,
            events,
            walk: CancelToken::new(),
            delivered: Mutex::new(HashMap::new()),
            revision: 0,
        });
        self.docs.lock().insert(doc, Arc::clone(&d));
        self.start_walk(&d, pool.clone());
        self.account_workers(pool);
        Ok(PdfOpened {
            doc,
            pages: info.pages,
            valid_xref: info.valid_xref,
            version: info.version,
            first_size,
            transport,
            open_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Asks for every page's size in batches, at Background priority, and
    /// streams them as `{kind: "sizes", from, sizes: [w, h, ...]}`.
    fn start_walk(&self, d: &Arc<PdfDoc>, pool: Pool) {
        let d = Arc::clone(d);
        let walk = d.walk.clone();
        let spec = JobSpec::new("pdf-page-walk", Priority::Background);
        drop(self.scheduler.submit(spec, move |_ctx| {
            let started = Instant::now();
            let mut from = 0;
            while from < d.pages {
                if walk.is_cancelled() {
                    return Ok(());
                }
                let to = (from + WALK_BATCH).min(d.pages);
                match pool.page_sizes(d.pool_doc, from, to) {
                    Ok(sizes) => {
                        {
                            let mut known = d.sizes.lock();
                            for (i, s) in sizes.iter().enumerate() {
                                known[from as usize + i] = Some(*s);
                            }
                        }
                        let flat: Vec<f32> = sizes.iter().flat_map(|&(w, h)| [w, h]).collect();
                        let _ = d
                            .events
                            .send(json!({"kind": "sizes", "from": from, "sizes": flat}));
                    }
                    Err(e) => {
                        tracing::warn!(%e, from, "page walk failed");
                        let _ = d.events.send(
                            json!({"kind": "walkFailed", "from": from, "error": e.to_string()}),
                        );
                        return Ok(());
                    }
                }
                from = to;
            }
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            tracing::info!(pages = d.pages, ms, "page walk done");
            let _ = d.events.send(json!({"kind": "sizesDone", "ms": ms}));
            Ok(())
        }));
    }

    fn request_tiles(
        &self,
        doc: u64,
        scale: f32,
        rotation: u32,
        size: u32,
        tiles_wanted: &[PageTileRequest],
    ) -> CmdResult<()> {
        let d = self.doc(doc)?;
        let pool = self.pool()?.clone();
        let rot = Rotation::from_degrees(rotation).ok_or("rotation must be a multiple of 90")?;
        if !(scale.is_finite() && scale > 0.0 && scale <= 200.0) || !(64..=1024).contains(&size) {
            return Err("bad tile request".into());
        }
        d.generation.bump();
        let layout = tiles::layout_id(scale, rot, d.revision);
        let now = Instant::now();
        for t in tiles_wanted {
            if t.page >= d.pages {
                continue;
            }
            let key = TileKey {
                view: doc,
                layout,
                col: t.col,
                row: tiles::row_id(t.page, t.row),
                size,
            };
            if recently_delivered(&d, &key, now) {
                continue;
            }
            let job_key = JobKey {
                kind: "pdf-tile",
                parts: [
                    doc,
                    layout,
                    (u64::from(t.col) << 32) | u64::from(size),
                    key.row,
                ],
            };
            let spec = JobSpec::new("pdf-tile", t.priority.into())
                .key(job_key)
                .generation(&d.generation);
            let d2 = Arc::clone(&d);
            let pool = pool.clone();
            let (page, col, row) = (t.page, t.col, t.row);
            drop(self.scheduler.submit(spec, move |ctx| {
                let page_size = d2.sizes.lock()[page as usize];
                let page_size = match page_size {
                    Some(s) => s,
                    None => match pool.page_sizes(d2.pool_doc, page, page + 1) {
                        Ok(s) => {
                            d2.sizes.lock()[page as usize] = Some(s[0]);
                            send_size(&d2, page, s[0]);
                            s[0]
                        }
                        Err(e) => {
                            tile_failed(&d2, page, &e);
                            return Ok(());
                        }
                    },
                };
                let px = tiles::page_pixels(page_size, rot, scale);
                let Some((w, h)) = tiles::tile_size(px, size, col, row) else {
                    // The view laid the page out with an estimated size:
                    // tell it the real one rather than leave a hole.
                    send_size(&d2, page, page_size);
                    return Ok(());
                };
                ctx.checkpoint()?;
                let matrix = tiles::tile_matrix(
                    page_size,
                    rot,
                    scale,
                    (col * size) as f32,
                    (row * size) as f32,
                );
                match pool.render(d2.pool_doc, page, matrix, w, h, protocol::flags::ANNOT) {
                    Ok(rgba) => {
                        ctx.checkpoint()?;
                        d2.delivered.lock().insert(key, Instant::now());
                        d2.sink.deliver(RenderedTile {
                            key,
                            width: w,
                            height: h,
                            rgba,
                        });
                    }
                    Err(e) => tile_failed(&d2, page, &e),
                }
                Ok(())
            }));
        }
        self.account_workers(&pool);
        Ok(())
    }

    fn close(&self, doc: u64) {
        if let Some(d) = self.docs.lock().remove(&doc) {
            d.walk.cancel();
            d.generation.bump();
            if let Some(Ok(pool)) = self.pool.get() {
                pool.close_document(d.pool_doc);
                self.account_workers(pool);
            }
        }
    }
}

fn send_size(d: &PdfDoc, page: u32, (w, h): (f32, f32)) {
    let _ = d
        .events
        .send(json!({"kind": "sizes", "from": page, "sizes": [w, h]}));
}

fn recently_delivered(d: &PdfDoc, key: &TileKey, now: Instant) -> bool {
    let mut delivered = d.delivered.lock();
    if delivered.len() > 4096 {
        delivered.retain(|_, at| now.duration_since(*at) < IN_FLIGHT_TTL);
    }
    delivered
        .get(key)
        .is_some_and(|at| now.duration_since(*at) < IN_FLIGHT_TTL)
}

/// A page that can't be rendered is labeled in the view, not retried
/// forever (§2: a visible placeholder, never a silent gap).
fn tile_failed(d: &PdfDoc, page: u32, e: &PdfError) {
    tracing::warn!(page, %e, "PDF tile failed");
    let _ = d
        .events
        .send(json!({"kind": "pageFailed", "page": page, "error": e.to_string()}));
}

fn open_error(e: PdfError) -> String {
    match e {
        PdfError::Password => "password".into(),
        other => other.to_string(),
    }
}

/// `pdf` when `%PDF-` occurs within the first 1 KiB (§5.3), else `text`.
pub fn probe(path: &Path) -> std::io::Result<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 1024];
    let mut f = std::fs::File::open(path)?;
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(if head[..n].windows(5).any(|w| w == b"%PDF-") {
        "pdf"
    } else {
        "text"
    })
}

#[tauri::command]
pub async fn doc_probe(path: String) -> CmdResult<&'static str> {
    probe(Path::new(&path)).map_err(|e| e.to_string())
}

/// The PDF from the command line, once.
#[tauri::command]
pub async fn pdf_startup(svc: State<'_, PdfService>) -> CmdResult<Option<String>> {
    Ok(svc.startup.lock().take())
}

#[tauri::command]
pub async fn pdf_open(
    window: tauri::WebviewWindow,
    svc: State<'_, PdfService>,
    path: String,
    password: Option<String>,
    tiles: Channel<InvokeResponseBody>,
    events: Channel<Value>,
) -> CmdResult<PdfOpened> {
    guarded(|| {
        let sink = crate::make_sink(&window, tiles);
        svc.open(&path, password.as_deref(), sink, events)
    })
}

#[tauri::command]
pub async fn pdf_request_tiles(
    svc: State<'_, PdfService>,
    doc: u64,
    scale: f32,
    rotation: u32,
    size: u32,
    tiles: Vec<PageTileRequest>,
) -> CmdResult<()> {
    guarded(|| svc.request_tiles(doc, scale, rotation, size, &tiles))
}

/// The worker pool's counters, for the HUD and tests.
#[tauri::command]
pub async fn pdf_stats(svc: State<'_, PdfService>) -> CmdResult<Option<quire_pdf::PoolStats>> {
    Ok(match svc.pool.get() {
        Some(Ok(pool)) => Some(pool.stats()),
        _ => None,
    })
}

/// Makes a worker abort, to test recovery. Refused unless the app was
/// started with `QUIRE_PDF_FAULTS=1`.
#[tauri::command]
pub async fn pdf_crash_worker(svc: State<'_, PdfService>) -> CmdResult<()> {
    if std::env::var_os("QUIRE_PDF_FAULTS").is_none_or(|v| v != "1") {
        return Err("fault injection is off".into());
    }
    svc.pool()?.crash_a_worker().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pdf_close(svc: State<'_, PdfService>, doc: u64) -> CmdResult<()> {
    svc.close(doc);
    Ok(())
}
