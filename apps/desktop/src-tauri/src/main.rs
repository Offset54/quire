// Release builds run as a GUI app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tracing_setup;

use quire_core::trace::ExternalEvent;
use tauri::{Manager, RunEvent};
use tracing_setup::Tracing;

/// Whether a Chrome trace is being recorded (set `QUIRE_TRACE=<file.json>`).
#[tauri::command]
fn trace_status(tracing: tauri::State<'_, Tracing>) -> bool {
    tracing.recorder.is_some()
}

/// Merges frontend timings (measures, long tasks) into the trace.
#[tauri::command]
fn trace_ingest(tracing: tauri::State<'_, Tracing>, events: Vec<ExternalEvent>) {
    if let Some(recorder) = &tracing.recorder {
        recorder.add_external(&events);
    }
}

fn main() {
    let tracing = tracing_setup::init();
    let startup = tracing::info_span!("startup").entered();

    let app = tauri::Builder::default()
        .manage(tracing)
        .invoke_handler(tauri::generate_handler![trace_status, trace_ingest])
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application");
    drop(startup);

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            handle.state::<Tracing>().write_trace();
        }
    });
}
