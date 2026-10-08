fn main() {
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "tile_invoke",
            "tile_channel",
            "tile_shared",
            "report",
        ]));
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
