// Release builds run as a GUI app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tracing_setup;

use quire_core::trace::ExternalEvent;
use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder};
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

/// The webview may only show the app's own pages (§8: no remote content).
/// Links to anything else are refused here; opening them in the user's
/// browser will be an explicit, confirmed action.
fn is_app_url(url: &Url) -> bool {
    match (url.scheme(), url.host_str()) {
        ("tauri", Some("localhost")) => true,
        ("http" | "https", Some("tauri.localhost")) => true,
        // Vite dev server (see tauri.conf.json `devUrl`).
        ("http", Some("localhost")) => cfg!(debug_assertions) && url.port() == Some(5173),
        _ => false,
    }
}

fn create_main_window(app: &tauri::App) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("quire")
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 400.0)
        .on_navigation(|url| {
            let allowed = is_app_url(url);
            if !allowed {
                tracing::warn!(%url, "blocked navigation");
            }
            allowed
        })
        .on_new_window(|url, _| {
            tracing::warn!(%url, "blocked new window");
            NewWindowResponse::Deny
        })
        .on_download(|_, _| false)
        .build()?;
    Ok(())
}

fn main() {
    let tracing = tracing_setup::init();
    let startup = tracing::info_span!("startup").entered();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .manage(tracing)
        .invoke_handler(tauri::generate_handler![trace_status, trace_ingest])
        .setup(|app| Ok(create_main_window(app)?))
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application");
    drop(startup);

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            handle.state::<Tracing>().write_trace();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(s: &str) -> bool {
        is_app_url(&Url::parse(s).unwrap())
    }

    #[test]
    fn only_app_origins_are_navigable() {
        assert!(allowed("http://tauri.localhost/index.html"));
        assert!(allowed("tauri://localhost/"));
        assert!(!allowed("https://example.com/"));
        assert!(!allowed("http://tauri.localhost.evil.com/"));
        assert!(!allowed("file:///C:/Windows/win.ini"));
        assert!(!allowed("http://localhost:8080/"));
        assert_eq!(allowed("http://localhost:5173/"), cfg!(debug_assertions));
    }
}
