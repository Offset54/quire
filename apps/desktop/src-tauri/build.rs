fn main() {
    let attributes = tauri_build::Attributes::new()
        // Only these app commands can be invoked, and only by windows whose
        // capability grants them (capabilities/main.json).
        .app_manifest(tauri_build::AppManifest::new().commands(&["trace_status", "trace_ingest"]))
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
