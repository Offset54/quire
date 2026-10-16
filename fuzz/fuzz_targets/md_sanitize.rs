//! Sanitization (M2 task 12, ADR-0028): any document, rendered through the
//! product pipeline with remote content off and on, and any fragment through
//! the sanitizer alone, passes the hostile-HTML check (a tokenizer written
//! apart from the sanitizer, shared with `tests/hostile.rs`): allowed tags
//! and attributes only, no event handlers or styles, no dangerous link
//! schemes, images only from inline data or our scheme.
//!
//! Input: the document (decoded lossily as UTF-8 for the fragment check).

#![no_main]

use std::path::PathBuf;

mod save_on_panic;

use libfuzzer_sys::fuzz_target;
use quire_markdown::document::render_document;
use quire_markdown::images::{ImagePolicy, ImageSrc};
use quire_markdown::render::RenderOptions;
use quire_markdown::sanitize::sanitize;

#[path = "../../crates/markdown/tests/support/html_check.rs"]
#[allow(dead_code)]
mod html_check;

use html_check::{SCHEME, check};

fuzz_target!(|data: &[u8]| {
    save_on_panic::guard("md_sanitize", data);
    for remote in [false, true] {
        let opts = RenderOptions {
            sanitize: Some(ImagePolicy {
                base_dir: Some(PathBuf::from("C:\\docs")),
                scheme: SCHEME.into(),
                remote: remote.then_some(1),
            }),
            ..RenderOptions::default()
        };
        let html = render_document(data, &opts);
        let bad = check("document", &html);
        assert!(bad.is_empty(), "{bad:?}\n{html}");
    }
    let s = String::from_utf8_lossy(data);
    let mut skip = None;
    let html = sanitize(
        &s,
        &mut |src| ImageSrc::Placeholder(src.to_owned()),
        &mut skip,
    );
    let bad = check("fragment", &html);
    assert!(bad.is_empty(), "{bad:?}\n{html}");
});
