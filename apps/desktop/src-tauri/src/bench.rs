//! In-app benchmark scenarios (M0 tasks 9 and 12). Compiled only with the
//! `bench-fixtures` feature.
//!
//! - `QUIRE_BENCH=<scenario>` makes the frontend run that scenario on start.
//! - `QUIRE_BENCH_OUT=<file.json>` is where `bench_report` writes results
//!   (default `bench-result.json`); the app exits afterwards.
//! - `QUIRE_BENCH_VIEWPORT=WxH` sets the window's logical size (ref-sim uses
//!   1280x720, i.e. 1920x1080 device px at 150 %, ADR-0021).

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{State, WebviewWindow, WebviewWindowBuilder, Wry};

use crate::synthetic::SyntheticList;
use crate::view::{ViewInfo, ViewService};

/// The fixed logical viewport requested with `QUIRE_BENCH_VIEWPORT=WxH`.
pub fn viewport_override() -> Option<(f64, f64)> {
    let v = std::env::var("QUIRE_BENCH_VIEWPORT").ok()?;
    let (w, h) = v.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

pub fn configure_window(
    builder: WebviewWindowBuilder<'_, Wry, tauri::App>,
) -> WebviewWindowBuilder<'_, Wry, tauri::App> {
    match viewport_override() {
        Some((w, h)) => builder.inner_size(w, h).resizable(false),
        None => builder,
    }
}

#[tauri::command]
pub fn bench_scenario() -> Option<String> {
    std::env::var("QUIRE_BENCH").ok()
}

#[tauri::command]
pub async fn bench_open_synthetic(
    window: WebviewWindow,
    views: State<'_, ViewService>,
    rows: u64,
    row_height: f64,
    width: u32,
    channel: Channel<InvokeResponseBody>,
) -> Result<ViewInfo, String> {
    let sink = crate::make_sink(&window, channel);
    Ok(views.open(Arc::new(SyntheticList::new(rows, row_height)), sink, width))
}

#[tauri::command]
pub async fn bench_report(app: tauri::AppHandle, results: serde_json::Value) -> Result<(), String> {
    let out = std::env::var("QUIRE_BENCH_OUT").unwrap_or_else(|_| "bench-result.json".into());
    let json = serde_json::to_vec_pretty(&results).map_err(|e| e.to_string())?;
    std::fs::write(&out, json).map_err(|e| e.to_string())?;
    tracing::info!(%out, "benchmark results written");
    app.exit(0);
    Ok(())
}
