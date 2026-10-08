fn main() {
    // Guarded copies out of mapped files (src/source/guard.c, ADR-0007).
    println!("cargo:rerun-if-changed=src/source/guard.c");
    cc::Build::new()
        .file("src/source/guard.c")
        .warnings(true)
        .compile("quire_guard");
}
