//! Hostile HTML (M2 task 4, ADR-0028): every file in
//! `tests/data/markdown/hostile/` is rendered through the product pipeline,
//! and the output is checked by a tokenizer written apart from the sanitizer,
//! following HTML5's attribute rules (which is what the webview applies):
//! only allowed tags and attributes, no event handlers or styles, no link
//! with a dangerous scheme, and no image that loads from anywhere but inline
//! data or our scheme.

use std::path::{Path, PathBuf};

use quire_markdown::document::render_document;
use quire_markdown::images::ImagePolicy;
use quire_markdown::render::RenderOptions;

#[path = "support/html_check.rs"]
mod html_check;

use html_check::{SCHEME, check};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/markdown/hostile")
}

fn render(bytes: &[u8], remote: bool) -> String {
    let opts = RenderOptions {
        sanitize: Some(ImagePolicy {
            base_dir: Some(PathBuf::from("C:\\docs")),
            scheme: SCHEME.into(),
            remote: remote.then_some(1),
        }),
        ..RenderOptions::default()
    };
    render_document(bytes, &opts)
}

#[test]
fn hostile_html_is_neutralized() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .expect("tests/data/markdown/hostile")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    assert!(files.len() >= 3);
    let mut bad = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).expect("read");
        let name = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for remote in [false, true] {
            let html = render(&bytes, remote);
            bad.extend(check(&name, &html));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn what_stays_and_what_is_a_placeholder() {
    let resources = std::fs::read(data_dir().join("resources.md")).expect("read");
    let off = render(&resources, false);
    assert!(off.contains("data-blocked-src=\"https://tracker.example/pixel.png\""));
    assert!(off.contains("data-blocked-src=\"https://tracker.example/p.png\""));
    assert!(off.contains("data-blocked-src=\"https://tracker.example/raw.png\""));
    assert!(off.contains("<a href=\"https://e.com/ok\""));
    let links = render(
        &std::fs::read(data_dir().join("links.md")).expect("read"),
        false,
    );
    // Markdown links keep the same allowlist as raw HTML, with `rel`.
    assert!(
        links.contains(" href=\"https://e.com/ok2\" rel=\"noopener noreferrer\">"),
        "{links}"
    );
    assert!(links.contains(" href=\"./other.md\""));
    assert!(links.contains(" href=\"#top\""));
    assert!(links.contains(" href=\"mailto:a@e.com\""));
    assert_eq!(links.matches(" href=\"\"").count(), 6, "{links}");
    assert!(off.contains("src=\"data:image/png;base64,iVBORw0KGgo=\""));
    assert!(off.contains("<input type=\"checkbox\" checked disabled>"));
    // `local.png` resolves against the document's folder.
    assert!(off.contains("data-src=\"http://quire-img.localhost/local/C%3A%5Cdocs"));
    assert!(off.contains("data-src=\"http://quire-img.localhost/local/C%3A%2FWindows%2Fwin.ini\""));
    assert!(!off.contains("/remote/"));
    let on = render(&resources, true);
    assert!(on.contains(
        "data-src=\"http://quire-img.localhost/remote/1/https%3A%2F%2Ftracker.example%2Fpixel.png\""
    ));
    // The forged `data-src` on a div is gone; on an img it is dropped too.
    assert!(!on.contains("C%3A%5Csecret.png"));
    assert!(!on.contains("forged.png"));
}

/// The check's own scheme rule (fuzz: md_sanitize, 2026-10-17): NULs in a
/// link's URL come out as `%EF%BF%BD`, and `%` can't be part of a scheme, so
/// the link is relative, as a browser parses it. Real schemes are still seen.
#[test]
fn the_check_reads_schemes_as_browsers_do() {
    let html = render(b"[x](http\0\0s://e.com/p)\n", false);
    assert!(
        html.contains(" href=\"http%EF%BF%BD%EF%BF%BDs://e.com/p\""),
        "{html}"
    );
    assert!(check("nul", &html).is_empty(), "{html}");
    for (href, ok) in [
        ("http%EF%BF%BDs://e.com", true),
        ("./a:b", true),
        ("1x:y", true),
        ("java\u{0}script:x", false),
        ("JaVa&#x09;Script:x", false),
        ("ms-settings:x", false),
        ("a+b.c-d:x", false),
    ] {
        let bad = check("href", &format!("<a href=\"{href}\">x</a>"));
        assert_eq!(bad.is_empty(), ok, "{href}: {bad:?}");
    }
}
