//! A PDFium worker process on its own (tests, benchmarks). The app runs the
//! same loop as `quire --pdf-worker` (ADR-0036).

fn main() {
    std::process::exit(quire_pdf::worker::run());
}
