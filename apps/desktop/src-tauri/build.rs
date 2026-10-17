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
        "memory_report",
        "memory_release",
        "txt_startup",
        "txt_open",
        "txt_new",
        "txt_close",
        "txt_view",
        "txt_edit",
        "txt_paste",
        "txt_copy",
        "txt_seek",
        "txt_goto_line",
        "txt_search",
        "txt_search_cancel",
        "txt_find",
        "txt_replace_all",
        "txt_cancel",
        "txt_open_cancel",
        "txt_save",
        "txt_set_format",
        "txt_reopen",
        "txt_allow_edit",
        "txt_status",
        "txt_check_disk",
        "txt_disk_resolve",
        "txt_recovery_list",
        "txt_recover",
        "txt_recovery_discard",
        "md_attach",
        "md_detach",
        "md_view",
        "md_set_remote",
        "md_status",
        "md_blocks",
        "md_outline",
        "md_table",
        "md_paste_image",
        "md_export_begin",
        "md_export_take",
        "md_export_put",
        "md_export_seal",
        "md_export_finish",
        "md_export_abort",
    ];
    if std::env::var_os("CARGO_FEATURE_BENCH_FIXTURES").is_some() {
        commands.extend([
            "bench_scenario",
            "bench_open_synthetic",
            "bench_report",
            "bench_env",
            "bench_txt_insert",
            "bench_memory",
            "bench_log",
        ]);
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
