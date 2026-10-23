//! The worker pool against the pinned PDFium (ADR-0035, ADR-0036).
//!
//! These tests need the library: `pwsh tools/pdfium/fetch-pdfium.ps1`, or
//! `QUIRE_PDFIUM` pointing at it. Without it they skip, unless
//! `QUIRE_PDFIUM_REQUIRED=1` (CI), where they fail.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pdf_writer::{Content, Finish, Pdf, Rect, Ref};
use quire_pdf::protocol::{Matrix, flags};
use quire_pdf::{PdfError, Pool, PoolConfig, WorkerCommand};

fn library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QUIRE_PDFIUM") {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (platform, lib) = if cfg!(windows) {
        ("windows-x64", "bin/pdfium.dll")
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            ("macos-arm64", "lib/libpdfium.dylib")
        } else {
            ("macos-x64", "lib/libpdfium.dylib")
        }
    } else {
        ("linux-x64", "lib/libpdfium.so")
    };
    let p = root
        .join(".cache/pdfium/151.0.7881.0")
        .join(platform)
        .join(lib);
    if p.exists() {
        return Some(p);
    }
    assert!(
        std::env::var_os("QUIRE_PDFIUM_REQUIRED").is_none(),
        "PDFium not found at {} (QUIRE_PDFIUM_REQUIRED is set)",
        p.display()
    );
    eprintln!("skipping: PDFium not fetched (pwsh tools/pdfium/fetch-pdfium.ps1)");
    None
}

fn pool(workers: usize) -> Option<Pool> {
    Some(Pool::new(PoolConfig {
        workers,
        command: WorkerCommand {
            program: PathBuf::from(env!("CARGO_BIN_EXE_quire-pdf-worker")),
            args: Vec::new(),
            library: library()?,
            faults: true,
        },
        timeout: Duration::from_secs(30),
    }))
}

/// `pages` A4 pages; page i has a filled square whose color encodes i.
fn test_pdf(pages: usize) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let ids: Vec<(Ref, Ref)> = (0..pages)
        .map(|i| (Ref::new(3 + 2 * i as i32), Ref::new(4 + 2 * i as i32)))
        .collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(ids.iter().map(|p| p.0))
        .count(pages as i32);
    for (i, &(page, content)) in ids.iter().enumerate() {
        let mut p = pdf.page(page);
        // Every third page is landscape.
        let size = if i % 3 == 2 {
            Rect::new(0.0, 0.0, 842.0, 595.0)
        } else {
            Rect::new(0.0, 0.0, 595.0, 842.0)
        };
        p.media_box(size).parent(tree).contents(content);
        p.finish();
        let mut c = Content::new();
        let (r, g, b) = color(i);
        c.set_fill_rgb(r, g, b);
        // 100 × 100 pt at (100, 100) from the top left.
        c.rect(100.0, size.y2 - 200.0, 100.0, 100.0);
        c.fill_nonzero();
        pdf.stream(content, &c.finish());
    }
    pdf.finish()
}

fn color(i: usize) -> (f32, f32, f32) {
    ((i % 4) as f32 / 3.0, ((i / 4) % 4) as f32 / 3.0, 0.5)
}

fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quire-pdf-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// The pixel at (x, y) of a `width`-wide RGBA bitmap.
fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let o = ((y * width + x) * 4) as usize;
    rgba[o..o + 4].try_into().unwrap()
}

fn expect_color(rgba: &[u8], width: u32, x: u32, y: u32, i: usize) {
    let (r, g, b) = color(i);
    let want = [
        (r * 255.0).round(),
        (g * 255.0).round(),
        (b * 255.0).round(),
    ];
    let got = px(rgba, width, x, y);
    for c in 0..3 {
        assert!(
            (f32::from(got[c]) - want[c]).abs() <= 2.0,
            "page {i} pixel ({x}, {y}): {got:?}, want {want:?}"
        );
    }
    assert_eq!(got[3], 255, "opaque");
}

#[test]
fn opens_measures_and_renders_pages() {
    let Some(pool) = pool(2) else { return };
    let path = write_temp("pages.pdf", &test_pdf(12));
    let (doc, info) = pool.open_document(path.to_str().unwrap(), None).unwrap();
    assert_eq!(info.pages, 12);
    assert!(info.valid_xref);

    let sizes = pool.page_sizes(doc, 0, 12).unwrap();
    assert_eq!(sizes[0], (595.0, 842.0));
    assert_eq!(sizes[2], (842.0, 595.0));

    // Whole page at 1 px per point: the square is at (100..200, 100..200).
    let rgba = pool
        .render(
            doc,
            5,
            Matrix::scale_translate(1.0, 0.0, 0.0),
            595,
            842,
            flags::ANNOT,
        )
        .unwrap();
    assert_eq!(rgba.len(), 595 * 842 * 4);
    expect_color(&rgba, 595, 150, 150, 5);
    assert_eq!(px(&rgba, 595, 50, 50), [255, 255, 255, 255], "white paper");

    // A 64 px tile at 8 px per point whose origin is inside the square.
    let rgba = pool
        .render(
            doc,
            7,
            Matrix::scale_translate(8.0, 1200.0, 1200.0),
            64,
            64,
            0,
        )
        .unwrap();
    expect_color(&rgba, 64, 0, 0, 7);
    expect_color(&rgba, 64, 63, 63, 7);

    // Errors: a page past the end, an unknown document.
    let e = pool
        .render(doc, 12, Matrix::scale_translate(1.0, 0.0, 0.0), 8, 8, 0)
        .unwrap_err();
    assert!(matches!(e, PdfError::Format { .. }), "{e:?}");
    pool.close_document(doc);
    assert_eq!(
        pool.render(doc, 0, Matrix::scale_translate(1.0, 0.0, 0.0), 8, 8, 0),
        Err(PdfError::UnknownDocument)
    );
}

#[test]
fn reports_unreadable_and_broken_files() {
    let Some(pool) = pool(1) else { return };
    let missing = std::env::temp_dir().join("quire-no-such-file.pdf");
    assert!(matches!(
        pool.open_document(missing.to_str().unwrap(), None),
        Err(PdfError::File(_))
    ));
    let junk = write_temp("junk.pdf", b"%PDF-1.7\nthis is not a PDF at all\n");
    assert!(matches!(
        pool.open_document(junk.to_str().unwrap(), None),
        Err(PdfError::Format { .. })
    ));
    // A wrong startxref: PDFium rebuilds the table and says so.
    let mut bytes = test_pdf(3);
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(at);
    bytes.extend_from_slice(b"startxref\n12\n%%EOF\n");
    let broken = write_temp("broken-xref.pdf", &bytes);
    let (_, info) = pool.open_document(broken.to_str().unwrap(), None).unwrap();
    assert_eq!(info.pages, 3);
    assert!(!info.valid_xref, "repaired");
}

#[test]
fn recovers_from_a_worker_crash() {
    let Some(pool) = pool(1) else { return };
    let path = write_temp("crash.pdf", &test_pdf(4));
    let (doc, _) = pool.open_document(path.to_str().unwrap(), None).unwrap();
    pool.render(doc, 0, Matrix::scale_translate(1.0, 0.0, 0.0), 300, 300, 0)
        .unwrap();
    pool.crash_a_worker().unwrap();
    // The next request finds the worker dead or sees it die, restarts it,
    // reopens the document there and renders.
    let rgba = pool
        .render(doc, 1, Matrix::scale_translate(1.0, 0.0, 0.0), 300, 300, 0)
        .unwrap();
    expect_color(&rgba, 300, 150, 150, 1);
    let s = pool.stats();
    assert!(s.spawned >= 2, "{s:?}");
    assert!(s.crashes >= 1, "{s:?}");
    assert_eq!(s.alive, 1);
}

#[test]
fn renders_concurrently_across_workers() {
    let Some(pool) = pool(3) else { return };
    let path = write_temp("concurrent.pdf", &test_pdf(30));
    let (doc, _) = pool.open_document(path.to_str().unwrap(), None).unwrap();
    let pool = Arc::new(pool);
    let threads: Vec<_> = (0..6)
        .map(|t| {
            let pool = Arc::clone(&pool);
            std::thread::spawn(move || {
                for i in (t..30).step_by(6) {
                    let rgba = pool
                        .render(
                            doc,
                            i as u32,
                            Matrix::scale_translate(2.0, 0.0, 0.0),
                            512,
                            512,
                            0,
                        )
                        .unwrap();
                    expect_color(&rgba, 512, 300, 300, i);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let s = pool.stats();
    assert_eq!(s.alive, 3, "{s:?}");
    assert_eq!(s.crashes, 0);
    assert!(s.private_bytes > 0 || !cfg!(any(windows, target_os = "linux")));
}

#[test]
fn an_overlay_is_appended_to_the_file() {
    let Some(pool) = pool(1) else { return };
    // An incremental update that replaces page 0's content with page 1's
    // stream: after it, page 0 renders in page 1's color.
    let base = test_pdf(2);
    let path = write_temp("overlay.pdf", &base);
    let (doc, _) = pool.open_document(path.to_str().unwrap(), None).unwrap();
    let start = base.windows(9).rposition(|w| w == b"startxref").unwrap();
    let prev: usize = std::str::from_utf8(&base[start + 9..])
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let obj_offset = base.len();
    let mut update =
        b"\n3 0 obj\n<< /Type /Page /MediaBox [0 0 595 842] /Parent 2 0 R /Contents 6 0 R >>\nendobj\n"
            .to_vec();
    let xref_offset = obj_offset + update.len();
    update.extend_from_slice(
        format!(
            "xref\n3 1\n{:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref_offset}\n%%EOF\n",
            obj_offset + 1
        )
        .as_bytes(),
    );
    let before = pool
        .render(doc, 0, Matrix::scale_translate(1.0, 0.0, 0.0), 300, 300, 0)
        .unwrap();
    expect_color(&before, 300, 150, 150, 0);
    pool.set_overlay(doc, update).unwrap();
    let after = pool
        .render(doc, 0, Matrix::scale_translate(1.0, 0.0, 0.0), 300, 300, 0)
        .unwrap();
    expect_color(&after, 300, 150, 150, 1);
}
