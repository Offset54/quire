//! Code highlighting (M2 task 12, ADR-0029, RISKS #16): every tree-sitter
//! grammar we ship (C parsers) gets arbitrary code. Highlighting must not
//! crash or hang, and its HTML must hold exactly the code's text: only our
//! own spans around it, everything else escaped. The Markdown inline grammar
//! is reached through the Markdown grammar's injections.
//!
//! Input: [grammar selector][code...] (decoded lossily as UTF-8).

#![no_main]

mod save_on_panic;

use libfuzzer_sys::fuzz_target;
use quire_highlight::{LANGUAGES, highlight};

/// The text of highlighted HTML: spans removed, entities decoded.
fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with("</span>") {
            rest = &rest[7..];
        } else if rest.starts_with("<span class=\"hl-") {
            let end = rest.find("\">").expect("an unterminated span");
            let class = &rest[16..end];
            assert!(
                class.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "bad class {class:?}"
            );
            rest = &rest[end + 2..];
        } else {
            let (entity, c) = [
                ("&lt;", '<'),
                ("&gt;", '>'),
                ("&amp;", '&'),
                ("&quot;", '"'),
            ]
            .into_iter()
            .find(|(e, _)| rest.starts_with(e))
            .unwrap_or_else(|| panic!("unescaped markup at {:?}", &rest[..rest.len().min(20)]));
            out.push(c);
            rest = &rest[entity.len()..];
        }
    }
    out.push_str(rest);
    out
}

fuzz_target!(|data: &[u8]| {
    save_on_panic::guard("highlight", data);
    let Some((&sel, code)) = data.split_first() else {
        return;
    };
    let shown: Vec<_> = LANGUAGES.iter().filter(|l| !l.hidden).collect();
    let lang = shown[usize::from(sel) % shown.len()];
    let code = String::from_utf8_lossy(code);
    if let Ok(html) = highlight(lang.names[0], &code) {
        assert_eq!(text_of(&html), code, "{}", lang.names[0]);
    }
});
