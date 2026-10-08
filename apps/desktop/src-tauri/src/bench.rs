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

/// Progress lines from a scenario, to stderr (diagnosing hangs).
#[tauri::command]
pub fn bench_log(message: String) {
    tracing::info!(target: "quire::bench", "{message}");
}

/// Benchmark parameters from the environment: the fixture to open
/// (`QUIRE_BENCH_FILE`) and a free-form phase (`QUIRE_BENCH_PHASE`).
#[tauri::command]
pub fn bench_env() -> serde_json::Value {
    serde_json::json!({
        "file": std::env::var("QUIRE_BENCH_FILE").ok(),
        "phase": std::env::var("QUIRE_BENCH_PHASE").ok(),
    })
}

/// Inserts `text` at each offset as a typed edit and returns the time each
/// took in the core, in microseconds (§4: insert at any offset).
#[tauri::command]
pub async fn bench_txt_insert(
    svc: State<'_, Arc<crate::txt::TxtService>>,
    doc: u64,
    offsets: Vec<u64>,
    text: String,
) -> Result<Vec<f64>, String> {
    offsets
        .into_iter()
        .map(|o| svc.bench_insert(doc, o, &text))
        .collect()
}

/// Process memory: private bytes (committed, not file-backed: the §4
/// "resident beyond the mapped source") and working set.
#[tauri::command]
pub fn bench_memory() -> serde_json::Value {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: zeroed counters are a valid out-parameter of the right size.
        let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: as above; the pseudo-handle is always valid.
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
                c.cb,
            )
        };
        if ok != 0 {
            return serde_json::json!({
                "privateBytes": c.PrivateUsage,
                "workingSet": c.WorkingSetSize,
                "peakWorkingSet": c.PeakWorkingSetSize,
            });
        }
    }
    serde_json::Value::Null
}

/// When the OS created this process, in Unix epoch milliseconds: the
/// starting point for startup timings.
fn process_created_epoch_ms() -> Option<f64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: valid out-parameters for the current process.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        // FILETIME counts 100 ns intervals since 1601-01-01.
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        Some((ticks as f64 / 10_000.0) - 11_644_473_600_000.0)
    }
    #[cfg(not(windows))]
    None
}

#[tauri::command]
pub async fn bench_report(
    app: tauri::AppHandle,
    mut results: serde_json::Value,
) -> Result<(), String> {
    let out = std::env::var("QUIRE_BENCH_OUT").unwrap_or_else(|_| "bench-result.json".into());
    if let Some(obj) = results.as_object_mut() {
        obj.insert(
            "processCreatedEpochMs".into(),
            process_created_epoch_ms().into(),
        );
        obj.insert(
            "cpuLogicalOverride".into(),
            std::env::var("QUIRE_CPU_LOGICAL").ok().into(),
        );
    }
    let json = serde_json::to_vec_pretty(&results).map_err(|e| e.to_string())?;
    std::fs::write(&out, json).map_err(|e| e.to_string())?;
    tracing::info!(%out, "benchmark results written");
    app.exit(0);
    Ok(())
}
