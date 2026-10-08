//! View service: one entry per open document surface. The frontend asks for
//! the tiles it lacks; each request bumps the view's generation, so queued
//! tiles that are no longer wanted are dropped before rendering, while tiles
//! that are still wanted coalesce with their pending jobs (§5.2, ADR-0005).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use quire_core::scheduler::{Generation, JobSpec, Priority, Scheduler};
use quire_core::tile::{Extent, TileKey, TileSource};
use serde::{Deserialize, Serialize};

use crate::transport::TileSink;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TilePriority {
    Visible,
    Near,
    Prefetch,
}

impl From<TilePriority> for Priority {
    fn from(p: TilePriority) -> Self {
        match p {
            TilePriority::Visible => Priority::Visible,
            TilePriority::Near => Priority::NearViewport,
            TilePriority::Prefetch => Priority::Prefetch,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TileRequest {
    pub col: u32,
    pub row: u64,
    pub priority: TilePriority,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewInfo {
    pub view: u64,
    pub layout: u64,
    pub extent: Extent,
    pub transport: &'static str,
}

/// How long a delivered tile is assumed to still be in flight to the
/// frontend. Re-requests inside this window are ignored, so a slow delivery
/// path isn't flooded with duplicate renders of the same tile; a tile that
/// really was lost is requested again after it expires.
const IN_FLIGHT_TTL: Duration = Duration::from_millis(400);

struct View {
    source: Arc<dyn TileSource>,
    sink: Arc<dyn TileSink>,
    generation: Generation,
    delivered: Mutex<HashMap<TileKey, Instant>>,
}

impl View {
    fn recently_delivered(&self, key: &TileKey, now: Instant) -> bool {
        let mut delivered = self.delivered.lock();
        if delivered.len() > 4096 {
            delivered.retain(|_, at| now.duration_since(*at) < IN_FLIGHT_TTL);
        }
        delivered
            .get(key)
            .is_some_and(|at| now.duration_since(*at) < IN_FLIGHT_TTL)
    }
}

pub struct ViewService {
    scheduler: Arc<Scheduler>,
    views: Mutex<HashMap<u64, Arc<View>>>,
    next_id: AtomicU64,
}

impl ViewService {
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self {
            scheduler,
            views: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn open(
        &self,
        source: Arc<dyn TileSource>,
        sink: Arc<dyn TileSink>,
        width: u32,
    ) -> ViewInfo {
        let view = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (layout, extent) = source.layout(width);
        let transport = sink.name();
        self.views.lock().insert(
            view,
            Arc::new(View {
                source,
                sink,
                generation: Generation::new(),
                delivered: Mutex::new(HashMap::new()),
            }),
        );
        ViewInfo {
            view,
            layout,
            extent,
            transport,
        }
    }

    fn get(&self, view: u64) -> Result<Arc<View>, String> {
        self.views
            .lock()
            .get(&view)
            .cloned()
            .ok_or_else(|| format!("no view {view}"))
    }

    pub fn relayout(&self, view: u64, width: u32) -> Result<ViewInfo, String> {
        let v = self.get(view)?;
        v.generation.bump();
        let (layout, extent) = v.source.layout(width);
        Ok(ViewInfo {
            view,
            layout,
            extent,
            transport: v.sink.name(),
        })
    }

    /// Replaces the view's wanted set with `tiles`.
    pub fn request(
        &self,
        view: u64,
        layout: u64,
        size: u32,
        tiles: &[TileRequest],
    ) -> Result<(), String> {
        let v = self.get(view)?;
        v.generation.bump();
        let now = Instant::now();
        for t in tiles {
            let key = TileKey {
                view,
                layout,
                col: t.col,
                row: t.row,
                size,
            };
            if v.recently_delivered(&key, now) {
                continue;
            }
            let view_ref = Arc::clone(&v);
            let spec = JobSpec::new("render-tile", t.priority.into())
                .key(key.job_key())
                .generation(&v.generation);
            // The handle is dropped: results go to the sink, and dropping
            // doesn't cancel (scheduler semantics).
            drop(self.scheduler.submit(spec, move |ctx| {
                let tile = view_ref.source.render(&key, ctx)?;
                ctx.checkpoint()?;
                view_ref.delivered.lock().insert(key, Instant::now());
                view_ref.sink.deliver(tile);
                Ok(())
            }));
        }
        Ok(())
    }

    pub fn close(&self, view: u64) {
        if let Some(v) = self.views.lock().remove(&view) {
            // Everything still queued for this view becomes stale.
            v.generation.bump();
        }
    }
}
