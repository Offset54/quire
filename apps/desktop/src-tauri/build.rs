fn main() {
    // App commands that can be invoked at all; each window still needs a
    // capability granting them (capabilities/, dev-capabilities/).
    let mut commands = vec![
        "trace_status",
        "trace_ingest",
        "view_request_tiles",
        "view_relayout",
        "view_close",
        "perf_stats",
    ];
    if std::env::var_os("CARGO_FEATURE_BENCH_FIXTURES").is_some() {
        commands.extend(["bench_scenario", "bench_open_synthetic", "bench_report"]);
    }
    let commands: &'static [&'static str] = Vec::leak(commands);

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(commands))
        // Windows manifest (kept plain ASCII; the SxS parser is strict):
        // - Common Controls v6, needed by Tauri's native dialogs;
        // - longPathAware, so paths over 260 characters work (§9);
        // - PerMonitorV2 DPI awareness for mixed-DPI setups (§3.1);
        // - UTF-8 active code page, so narrow (A) APIs in native libraries
        //   accept Unicode paths.
        .windows_attributes(
            tauri_build::WindowsAttributes::new()
                .app_manifest(include_str!("windows-app-manifest.xml")),
        );
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
