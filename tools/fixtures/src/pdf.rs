//! PDF fixtures: a 5,000-page ~1 GB book and a 1,000-page scanned book.
//! pdf-writer writes no timestamps or random IDs, so output is deterministic.

use std::path::Path;
use std::process::Command;

use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::images::{par_generate, photo_jpeg, scan_jpeg};
use crate::rng::{Rng, WORDS};

fn a4() -> Rect {
    Rect::new(0.0, 0.0, 595.0, 842.0)
}

/// Allocates object ids in order.
struct Ids(i32);

impl Ids {
    fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

/// ASCII-only line of words (the standard Helvetica font is WinAnsi).
fn ascii_line(rng: &mut Rng, len: usize) -> String {
    let mut s = String::new();
    while s.len() < len {
        let w = rng.pick(WORDS);
        if w.is_ascii() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(w);
        }
    }
    s
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Linearizes with qpdf when it is installed; returns details for the manifest.
fn linearize(path: &Path) -> serde_json::Value {
    let out = path.with_file_name(
        path.file_stem().unwrap().to_string_lossy().into_owned() + "-linearized.pdf",
    );
    // --deterministic-id keeps the output byte-identical between runs.
    let status = Command::new("qpdf")
        .args(["--linearize", "--deterministic-id"])
        .arg(path)
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() || s.code() == Some(3) => serde_json::json!({
            "file": out.file_name().unwrap().to_string_lossy(),
            "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
            "sha256": sha256_hex(&out).unwrap_or_default(),
        }),
        Ok(s) => serde_json::json!({ "skipped": format!("qpdf exited with {s}") }),
        Err(e) => serde_json::json!({ "skipped": format!("qpdf not available: {e}") }),
    }
}

pub fn book(path: &Path, seed: u64) -> std::io::Result<serde_json::Value> {
    const PAGES: usize = 5000;
    const PER_CHAPTER: usize = 50;
    let (iw, ih) = (800u32, 600u32);
    let images = par_generate(PAGES as u64, |i| photo_jpeg(seed, i, iw, ih, 48, 80));
    let mut rng = Rng::new(seed);

    let mut ids = Ids(0);
    let catalog = ids.next();
    let tree = ids.next();
    let font = ids.next();
    let outline = ids.next();
    let info = ids.next();
    let pages: Vec<(Ref, Ref, Ref)> = (0..PAGES)
        .map(|_| (ids.next(), ids.next(), ids.next()))
        .collect();
    let chapters: Vec<Ref> = (0..PAGES / PER_CHAPTER).map(|_| ids.next()).collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree).outlines(outline);
    pdf.pages(tree)
        .kids(pages.iter().map(|p| p.0))
        .count(PAGES as i32);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    pdf.document_info(info)
        .title(TextStr("5,000-page PDF fixture"))
        .producer(TextStr("quire-fixtures"));

    for (i, (&(page_id, content_id, image_id), jpeg)) in pages.iter().zip(&images).enumerate() {
        let mut page = pdf.page(page_id);
        page.media_box(a4()).parent(tree).contents(content_id);
        let mut res = page.resources();
        res.fonts().pair(Name(b"F1"), font);
        res.x_objects().pair(Name(b"Im1"), image_id);
        res.finish();
        page.finish();

        let mut img = pdf.image_xobject(image_id, jpeg);
        img.filter(Filter::DctDecode);
        img.width(iw as i32);
        img.height(ih as i32);
        img.bits_per_component(8);
        img.color_space().device_rgb();
        img.finish();

        let n = i + 1;
        let mut c = Content::new();
        c.begin_text();
        c.set_font(Name(b"F1"), 16.0);
        c.next_line(56.0, 780.0);
        c.show(Str(format!("Page {n} - token-{n:05}").as_bytes()));
        c.set_font(Name(b"F1"), 10.0);
        c.set_leading(13.0);
        for _ in 0..30 {
            c.next_line_using_leading();
            let len = rng.range(60, 95) as usize;
            c.show(Str(ascii_line(&mut rng, len).as_bytes()));
        }
        c.end_text();
        c.save_state();
        c.transform([400.0, 0.0, 0.0, 300.0, 97.5, 60.0]);
        c.x_object(Name(b"Im1"));
        c.restore_state();
        pdf.stream(content_id, &c.finish());
    }

    // Outline: one entry per chapter of 50 pages.
    pdf.outline(outline)
        .first(chapters[0])
        .last(*chapters.last().unwrap())
        .count(chapters.len() as i32);
    for (k, &item) in chapters.iter().enumerate() {
        let title = format!(
            "Chapter {} (pages {}-{})",
            k + 1,
            k * PER_CHAPTER + 1,
            (k + 1) * PER_CHAPTER
        );
        let mut entry = pdf.outline_item(item);
        entry.title(TextStr(&title)).parent(outline);
        if k > 0 {
            entry.prev(chapters[k - 1]);
        }
        if k + 1 < chapters.len() {
            entry.next(chapters[k + 1]);
        }
        entry.dest().page(pages[k * PER_CHAPTER].0).fit();
        entry.finish();
    }

    std::fs::write(path, pdf.finish())?;
    Ok(serde_json::json!({
        "pages": PAGES, "imagesPerPage": 1, "imageSize": [iw, ih],
        "outlineEntries": chapters.len(), "linearized": linearize(path),
        "searchToken": "each page contains 'token-NNNNN' with its page number"
    }))
}

pub fn scanned(path: &Path, seed: u64) -> std::io::Result<serde_json::Value> {
    const PAGES: usize = 1000;
    // A4 at 200 dpi.
    let (iw, ih) = (1654u32, 2339u32);
    let images = par_generate(PAGES as u64, |i| scan_jpeg(seed, i, iw, ih));

    let mut ids = Ids(0);
    let catalog = ids.next();
    let tree = ids.next();
    let pages: Vec<(Ref, Ref, Ref)> = (0..PAGES)
        .map(|_| (ids.next(), ids.next(), ids.next()))
        .collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(pages.iter().map(|p| p.0))
        .count(PAGES as i32);
    for (&(page_id, content_id, image_id), jpeg) in pages.iter().zip(&images) {
        let mut page = pdf.page(page_id);
        page.media_box(a4()).parent(tree).contents(content_id);
        page.resources().x_objects().pair(Name(b"Scan"), image_id);
        page.finish();
        let mut img = pdf.image_xobject(image_id, jpeg);
        img.filter(Filter::DctDecode);
        img.width(iw as i32);
        img.height(ih as i32);
        img.bits_per_component(8);
        img.color_space().device_gray();
        img.finish();
        let mut c = Content::new();
        c.save_state();
        c.transform([595.0, 0.0, 0.0, 842.0, 0.0, 0.0]);
        c.x_object(Name(b"Scan"));
        c.restore_state();
        pdf.stream(content_id, &c.finish());
    }
    std::fs::write(path, pdf.finish())?;
    Ok(serde_json::json!({ "pages": PAGES, "dpi": 200, "imageSize": [iw, ih], "textLayer": false }))
}
