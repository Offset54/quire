//! M0 task 7 spike (ADR-0004): how fast can a rendered tile get from the Rust
//! core onto the webview's canvas? The frontend (../src/main.ts) runs every
//! scenario, posts the results to `report`, and the app writes them to
//! `SPIKE_OUT` (default `spike-tile-transport.json`) and exits.
//!
//! Transports measured:
//! - `uri`: async custom URI scheme (`http://tile.localhost/...` on Windows)
//! - `invoke`: async command returning a raw binary `Response`
//! - `channel`: async command pushing raw bytes through a `Channel`
//! - `shared`: WebView2 SharedBuffer posted to script (Windows only)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tiles;

use std::sync::Arc;

use serde::Deserialize;
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{State, WebviewUrl, WebviewWindowBuilder};
use tiles::{Content, Encoding, Store};

#[derive(Debug, Clone, Copy, Deserialize)]
struct TileReq {
    encoding: Encoding,
    content: Content,
    size: u32,
    index: u64,
}

fn parse_uri_path(path: &str) -> Option<TileReq> {
    // /{encoding}/{content}/{size}/{index}
    let mut parts = path.trim_start_matches('/').split('/');
    let quoted = |s: &str| format!("\"{s}\"");
    Some(TileReq {
        encoding: serde_json::from_str(&quoted(parts.next()?)).ok()?,
        content: serde_json::from_str(&quoted(parts.next()?)).ok()?,
        size: parts.next()?.parse().ok()?,
        index: parts.next()?.parse().ok()?,
    })
}

#[tauri::command]
async fn tile_invoke(store: State<'_, Arc<Store>>, req: TileReq) -> Result<Response, ()> {
    let bytes = store.get(req.encoding, req.content, req.size, req.index);
    Ok(Response::new(bytes.to_vec()))
}

/// Pushes `[u32 id LE][tile bytes]` through the channel.
#[tauri::command]
async fn tile_channel(
    store: State<'_, Arc<Store>>,
    req: TileReq,
    id: u32,
    channel: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let bytes = store.get(req.encoding, req.content, req.size, req.index);
    let mut payload = Vec::with_capacity(4 + bytes.len());
    payload.extend_from_slice(&id.to_le_bytes());
    payload.extend_from_slice(&bytes);
    channel
        .send(InvokeResponseBody::Raw(payload))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn tile_shared(
    window: tauri::WebviewWindow,
    store: State<'_, Arc<Store>>,
    req: TileReq,
    id: u32,
) -> Result<(), String> {
    let bytes = store.get(req.encoding, req.content, req.size, req.index);
    #[cfg(windows)]
    {
        window
            .with_webview(move |webview| {
                // SAFETY: runs on the UI thread that owns the WebView2 objects.
                if let Err(err) = unsafe { shared::post(&webview, &bytes, id) } {
                    eprintln!("PostSharedBufferToScript failed: {err}");
                }
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (window, bytes, id);
        Err("SharedBuffer is WebView2-only".into())
    }
}

#[cfg(windows)]
mod shared {
    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY, ICoreWebView2_17, ICoreWebView2Environment12,
    };
    use windows_core::{HSTRING, Interface};

    /// # Safety
    /// Must run on the webview's UI thread.
    pub unsafe fn post(
        webview: &PlatformWebview,
        bytes: &[u8],
        id: u32,
    ) -> windows_core::Result<()> {
        // SAFETY: COM calls on live interfaces owned by this webview, on its
        // UI thread; the copy stays inside the buffer's reported size.
        unsafe {
            let env: ICoreWebView2Environment12 = webview.environment().cast()?;
            let buffer = env.CreateSharedBuffer(bytes.len() as u64)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Buffer(&mut ptr)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let core: ICoreWebView2_17 = webview.controller().CoreWebView2()?.cast()?;
            let json = HSTRING::from(format!("{{\"id\":{id}}}"));
            core.PostSharedBufferToScript(
                &buffer,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
                &json,
            )
        }
    }
}

/// Progress lines from the runner, so a stuck run shows where it stopped.
#[tauri::command]
fn progress(message: String) {
    eprintln!("[spike] {message}");
    let log =
        std::env::var("SPIKE_OUT").unwrap_or_else(|_| "spike-tile-transport.json".into()) + ".log";
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        use std::io::Write;
        let _ = writeln!(f, "{message}");
    }
}

#[tauri::command]
async fn report(
    app: tauri::AppHandle,
    store: State<'_, Arc<Store>>,
    results: serde_json::Value,
) -> Result<(), String> {
    let out = std::env::var("SPIKE_OUT").unwrap_or_else(|_| "spike-tile-transport.json".into());
    let doc = serde_json::json!({ "tileSizes": store.sizes(), "results": results });
    std::fs::write(
        &out,
        serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    eprintln!("spike results written to {out}");
    app.exit(0);
    Ok(())
}

fn main() {
    let store = Arc::new(Store::generate());
    let uri_store = Arc::clone(&store);

    tauri::Builder::default()
        .manage(store)
        .register_asynchronous_uri_scheme_protocol("tile", move |_ctx, request, responder| {
            let store = Arc::clone(&uri_store);
            let path = request.uri().path().to_owned();
            // Produce the body off the UI thread, as the real core would.
            std::thread::spawn(move || {
                let response = match parse_uri_path(&path) {
                    Some(r) => tauri::http::Response::builder()
                        .header("Content-Type", "application/octet-stream")
                        .header("Access-Control-Allow-Origin", "*")
                        .body(store.get(r.encoding, r.content, r.size, r.index).to_vec()),
                    None => tauri::http::Response::builder()
                        .status(400)
                        .body(Vec::new()),
                };
                responder.respond(response.expect("valid response"));
            });
        })
        .invoke_handler(tauri::generate_handler![
            tile_invoke,
            tile_channel,
            tile_shared,
            progress,
            report
        ])
        .setup(|app| {
            // 1280×720 logical at 150 % = the reference machine's 1920×1080.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("tile transport spike")
                .inner_size(1280.0, 720.0)
                .resizable(false)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run spike");
}
