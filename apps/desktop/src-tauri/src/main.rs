// Release builds run as a GUI app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "bench-fixtures")]
mod bench;
mod frontend_cache;
mod images;
mod md;
#[cfg(feature = "bench-fixtures")]
mod synthetic;
mod tracing_setup;
mod txt;
// Tile views: only the bench-fixtures build opens one today (TXT renders
// DOM rows, ADR-0023).
#[cfg_attr(not(feature = "bench-fixtures"), allow(dead_code))]
mod transport;
#[cfg_attr(not(feature = "bench-fixtures"), allow(dead_code))]
mod view;

use std::sync::Arc;

use frontend_cache::{EventSink, FrontendCaches};
use quire_core::memory::{GovernorConfig, GovernorStats, MemoryGovernor};
use quire_core::scheduler::{JobRecord, Scheduler, SchedulerConfig, SchedulerStats};
use quire_core::trace::ExternalEvent;
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, State, Url, WebviewUrl, WebviewWindowBuilder};
use tracing_setup::Tracing;
use transport::{ChannelSink, TileSink};
use txt::TxtService;
use view::{TileRequest, ViewInfo, ViewService};

/// Whether a Chrome trace is being recorded (set `QUIRE_TRACE=<file.json>`).
#[tauri::command]
fn trace_status(tracing: State<'_, Tracing>) -> bool {
    tracing.recorder.is_some()
}

/// Merges frontend timings (measures, long tasks) into the trace.
#[tauri::command]
fn trace_ingest(tracing: State<'_, Tracing>, events: Vec<ExternalEvent>) {
    if let Some(recorder) = &tracing.recorder {
        recorder.add_external(&events);
    }
}

// Commands that touch the scheduler are async so they run off the UI thread.

#[tauri::command]
async fn view_request_tiles(
    views: State<'_, ViewService>,
    view: u64,
    layout: u64,
    size: u32,
    tiles: Vec<TileRequest>,
) -> Result<(), String> {
    views.request(view, layout, size, &tiles)
}

#[tauri::command]
async fn view_relayout(
    views: State<'_, ViewService>,
    view: u64,
    width: u32,
) -> Result<ViewInfo, String> {
    views.relayout(view, width)
}

#[tauri::command]
async fn view_close(views: State<'_, ViewService>, view: u64) -> Result<(), String> {
    views.close(view);
    Ok(())
}

/// The size of a webview cache (`tile-bitmaps` per view), so it counts
/// against the memory governor (§5.5).
#[tauri::command]
async fn memory_report(
    caches: State<'_, FrontendCaches>,
    cache: String,
    key: u64,
    doc: Option<u64>,
    bytes: u64,
) -> Result<(), String> {
    caches.report(&cache, key, doc, bytes)
}

#[tauri::command]
async fn memory_release(
    caches: State<'_, FrontendCaches>,
    cache: String,
    key: u64,
) -> Result<(), String> {
    caches.release(&cache, key);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PerfStats {
    scheduler: SchedulerStats,
    recent_jobs: Vec<JobRecord>,
    memory: GovernorStats,
}

/// Numbers for the performance HUD (§7).
#[tauri::command]
async fn perf_stats(
    views: State<'_, ViewService>,
    governor: State<'_, MemoryGovernor>,
) -> Result<PerfStats, String> {
    Ok(PerfStats {
        scheduler: views.scheduler().stats(),
        recent_jobs: views.scheduler().recent_jobs(),
        memory: governor.stats(),
    })
}

#[cfg_attr(not(feature = "bench-fixtures"), allow(dead_code))]
/// Picks the tile transport for a view (ADR-0004). `QUIRE_TRANSPORT=channel`
/// forces the cross-platform path.
fn make_sink(
    window: &tauri::WebviewWindow,
    channel: Channel<InvokeResponseBody>,
) -> Arc<dyn TileSink> {
    let fallback = ChannelSink::new(channel);
    let forced = std::env::var("QUIRE_TRANSPORT").is_ok_and(|v| v == "channel");
    #[cfg(windows)]
    if !forced {
        return Arc::new(transport::SharedBufferSink::new(window.clone(), fallback));
    }
    let _ = (window, forced);
    Arc::new(fallback)
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

/// Under tauri-driver, msedgedriver passes its switches (among them
/// `--remote-debugging-port`) in `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`.
/// On the GitHub Windows runner (elevated, WebView2 153) the runtime ignores
/// that variable, so no DevTools port opens and msedgedriver times out on
/// `DevToolsActivePort`. Under automation the app therefore passes the
/// switches itself, together with wry's defaults. Chromium keeps only the
/// last `--disable-features`, so the two feature lists are merged.
#[cfg(windows)]
fn webdriver_browser_args() -> Option<String> {
    if std::env::var_os("TAURI_WEBVIEW_AUTOMATION").is_none_or(|v| v != "true") {
        return None;
    }
    let driver = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").ok()?;
    let mut features = String::from("msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    let mut args = vec![String::from("--autoplay-policy=no-user-gesture-required")];
    for arg in driver.split_whitespace() {
        match arg.strip_prefix("--disable-features=") {
            Some(f) => {
                features.push(',');
                features.push_str(f);
            }
            None => args.push(arg.to_owned()),
        }
    }
    tracing::info!(%driver, "WebDriver browser arguments applied");
    Some(format!("--disable-features={features} {}", args.join(" ")))
}

fn create_main_window(app: &tauri::App) -> tauri::Result<()> {
    let _span = tracing::info_span!("create_main_window").entered();
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("quire")
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 400.0);
    #[cfg(feature = "bench-fixtures")]
    let builder = bench::configure_window(builder);
    #[cfg(windows)]
    let builder = match webdriver_browser_args() {
        Some(args) => builder.additional_browser_args(&args),
        None => builder,
    };
    builder
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

    let scheduler = tracing::info_span!("init_scheduler")
        .in_scope(|| Arc::new(Scheduler::new(SchedulerConfig::default())));
    let governor = MemoryGovernor::new(GovernorConfig::default());

    // A file on the command line starts opening now, in parallel with
    // WebView2 startup (ADR-0022).
    let txt = TxtService::new(Arc::clone(&scheduler), governor.clone());
    // Only an existing file counts. Under WebDriver, msedgedriver passes the
    // app's arguments as `--<argument>` switches, so that form is accepted
    // too when it names a file.
    let file_arg = std::env::args_os().skip(1).find_map(|a| {
        let p = std::path::PathBuf::from(&a);
        if p.is_file() {
            return Some(p);
        }
        let s = a.to_string_lossy();
        let q = std::path::PathBuf::from(s.strip_prefix("--")?);
        q.is_file().then_some(q)
    });
    tracing::info!(args = ?std::env::args_os().skip(1).collect::<Vec<_>>(), file = ?file_arg, "command line");
    if let Some(path) = file_arg {
        txt.preopen(path);
    }

    // Benchmarks fix the window size; a restored size would override it.
    #[cfg(feature = "bench-fixtures")]
    let restore_window = bench::viewport_override().is_none();
    #[cfg(not(feature = "bench-fixtures"))]
    let restore_window = true;

    let mut builder = tauri::Builder::default();
    if restore_window {
        builder = builder.plugin(tauri_plugin_window_state::Builder::new().build());
    }
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    let image_service = images::ImageService::new(&governor);
    let served = Arc::clone(&image_service);
    let builder = builder.register_asynchronous_uri_scheme_protocol(
        images::SCHEME,
        move |_ctx, request, responder| served.handle(request, move |r| responder.respond(r)),
    );
    let markdown = md::MdService::new(
        Arc::clone(&txt),
        Arc::clone(&image_service),
        governor.clone(),
    );
    let builder = builder
        .manage(markdown)
        .manage(image_service)
        .manage(tracing)
        .manage(ViewService::new(scheduler))
        .manage(governor.clone())
        .manage(txt);

    #[cfg(feature = "bench-fixtures")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        trace_status,
        trace_ingest,
        view_request_tiles,
        view_relayout,
        view_close,
        perf_stats,
        memory_report,
        memory_release,
        txt::txt_startup,
        txt::txt_open,
        txt::txt_new,
        txt::txt_close,
        txt::txt_view,
        txt::txt_edit,
        txt::txt_paste,
        txt::txt_copy,
        txt::txt_seek,
        txt::txt_goto_line,
        txt::txt_search,
        txt::txt_search_cancel,
        txt::txt_find,
        txt::txt_replace_all,
        txt::txt_cancel,
        txt::txt_open_cancel,
        txt::txt_save,
        txt::txt_set_format,
        txt::txt_reopen,
        txt::txt_allow_edit,
        txt::txt_status,
        txt::txt_check_disk,
        txt::txt_disk_resolve,
        txt::txt_recovery_list,
        txt::txt_recover,
        txt::txt_recovery_discard,
        md::md_attach,
        md::md_detach,
        md::md_view,
        md::md_set_remote,
        md::md_status,
        bench::bench_scenario,
        bench::bench_open_synthetic,
        bench::bench_report,
        bench::bench_env,
        bench::bench_txt_insert,
        bench::bench_memory,
        bench::bench_log,
    ]);
    #[cfg(not(feature = "bench-fixtures"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        trace_status,
        trace_ingest,
        view_request_tiles,
        view_relayout,
        view_close,
        perf_stats,
        memory_report,
        memory_release,
        txt::txt_startup,
        txt::txt_open,
        txt::txt_new,
        txt::txt_close,
        txt::txt_view,
        txt::txt_edit,
        txt::txt_paste,
        txt::txt_copy,
        txt::txt_seek,
        txt::txt_goto_line,
        txt::txt_search,
        txt::txt_search_cancel,
        txt::txt_find,
        txt::txt_replace_all,
        txt::txt_cancel,
        txt::txt_open_cancel,
        txt::txt_save,
        txt::txt_set_format,
        txt::txt_reopen,
        txt::txt_allow_edit,
        txt::txt_status,
        txt::txt_check_disk,
        txt::txt_disk_resolve,
        txt::txt_recovery_list,
        txt::txt_recover,
        txt::txt_recovery_discard,
        md::md_attach,
        md::md_detach,
        md::md_view,
        md::md_set_remote,
        md::md_status,
    ]);

    let build_span = tracing::info_span!("tauri_build").entered();
    let app = builder
        .on_page_load(|_, payload| {
            tracing::info!(event = ?payload.event(), url = %payload.url(), "page load");
        })
        .setup(move |app| {
            let sink = Arc::new(EventSink(app.handle().clone()));
            app.manage(FrontendCaches::new(governor, sink));
            #[cfg(feature = "bench-fixtures")]
            app.add_capability(include_str!("../dev-capabilities/bench.json"))?;
            Ok(create_main_window(app)?)
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application");
    drop(build_span);
    drop(startup);

    app.run(|handle, event| match event {
        RunEvent::Ready => tracing::info!("event loop ready"),
        RunEvent::Exit => {
            txt::shutdown(&handle.state::<Arc<TxtService>>());
            handle.state::<Tracing>().write_trace();
        }
        _ => {}
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
