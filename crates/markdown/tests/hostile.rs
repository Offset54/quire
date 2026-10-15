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

const SCHEME: &str = "http://quire-img.localhost/";

/// Tags the renderer or the sanitizer may emit.
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

const ATTRS: &[&str] = &[
    "align",
    "alt",
    "border",
    "cellpadding",
    "cellspacing",
    "colspan",
    "datetime",
    "dir",
    "headers",
    "height",
    "hspace",
    "lang",
    "rowspan",
    "scope",
    "span",
    "start",
    "summary",
    "title",
    "type",
    "valign",
    "vspace",
    "width",
    "open",
    "reversed",
    "cite",
    "abbr",
    "href",
    "src",
    "data-src",
    "data-blocked-src",
    "data-sourcepos",
    "id",
    "class",
    "rel",
    "disabled",
    "checked",
    "data-footnote-ref",
    "data-footnotes",
    "data-footnote-backref",
    "data-footnote-backref-idx",
    "aria-label",
    "data-math-style",
    // Code blocks (ADR-0029): the fence's language, escaped, and why it is plain.
    "data-lang",
    "data-hl",
];

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/markdown/hostile")
}

/// HTML5 character references in attribute values, including numeric ones
/// without a semicolon (browsers decode those).
fn decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' && b.get(i + 1) == Some(&b'#') {
            let mut j = i + 2;
            let hexa = matches!(b.get(j), Some(b'x' | b'X'));
            if hexa {
                j += 1;
            }
            let start = j;
            while j < b.len()
                && (if hexa {
                    b[j].is_ascii_hexdigit()
                } else {
                    b[j].is_ascii_digit()
                })
            {
                j += 1;
            }
            if j > start {
                let n =
                    u32::from_str_radix(&v[start..j], if hexa { 16 } else { 10 }).unwrap_or(0xfffd);
                out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                if b.get(j) == Some(&b';') {
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        if b[i] == b'&' {
            let rest = &v[i..];
            let named = [
                ("&amp;", '&'),
                ("&lt;", '<'),
                ("&gt;", '>'),
                ("&quot;", '"'),
                ("&#39;", '\''),
                ("&colon;", ':'),
                ("&tab;", '\t'),
                ("&newline;", '\n'),
            ];
            if let Some((n, c)) = named.iter().find(|(n, _)| rest.starts_with(n)) {
                out.push(*c);
                i += n.len();
                continue;
            }
        }
        let c = v[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[derive(Debug)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
}

/// Start tags in `html`, as an HTML5 tokenizer sees them in the data state.
fn start_tags(html: &str) -> Vec<Tag> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        i += 1;
        if i >= b.len() {
            break;
        }
        if b[i] == b'!' || b[i] == b'?' {
            // Comments, doctypes: emitted HTML has none (checked below).
            i += 1;
            continue;
        }
        let end_tag = b[i] == b'/';
        if end_tag {
            i += 1;
        }
        if i >= b.len() || !b[i].is_ascii_alphabetic() {
            continue;
        }
        let s = i;
        while i < b.len() && !matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ' | b'/' | b'>') {
            i += 1;
        }
        let name = html[s..i].to_ascii_lowercase();
        let mut attrs = Vec::new();
        loop {
            while i < b.len() && matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ' | b'/') {
                i += 1;
            }
            if i >= b.len() || b[i] == b'>' {
                i += 1;
                break;
            }
            let s = i;
            i += 1;
            while i < b.len()
                && !matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ' | b'/' | b'>' | b'=')
            {
                i += 1;
            }
            let an = html[s..i].to_ascii_lowercase();
            while i < b.len() && matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ') {
                i += 1;
            }
            let mut value = String::new();
            if b.get(i) == Some(&b'=') {
                i += 1;
                while i < b.len() && matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ') {
                    i += 1;
                }
                match b.get(i) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let e = html[i + 1..].find(q as char).map_or(b.len(), |e| i + 1 + e);
                        value = decode(&html[i + 1..e]);
                        i = e + 1;
                    }
                    _ => {
                        let s = i;
                        while i < b.len() && !matches!(b[i], b'\t' | b'\n' | b'\x0c' | b' ' | b'>')
                        {
                            i += 1;
                        }
                        value = decode(&html[s..i]);
                    }
                }
            }
            attrs.push((an, value));
        }
        if !end_tag {
            out.push(Tag { name, attrs });
        }
    }
    out
}

fn scheme(url: &str) -> Option<String> {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    let i = cleaned.find(':')?;
    (!cleaned[..i].contains(['/', '?', '#'])).then(|| cleaned[..i].to_owned())
}

fn check(name: &str, html: &str) -> Vec<String> {
    let mut bad = Vec::new();
    if html.contains("<!--") || html.to_ascii_lowercase().contains("<!doctype") {
        bad.push(format!("{name}: comment or doctype in output"));
    }
    for t in start_tags(html) {
        if !TAGS.contains(&t.name.as_str()) {
            bad.push(format!("{name}: tag <{}>", t.name));
        }
        for (k, v) in &t.attrs {
            if !ATTRS.contains(&k.as_str()) {
                bad.push(format!("{name}: <{} {k}={v:?}>", t.name));
            }
            match (t.name.as_str(), k.as_str()) {
                ("a", "href") => {
                    if let Some(s) = scheme(v)
                        && !matches!(s.as_str(), "http" | "https" | "mailto")
                    {
                        bad.push(format!("{name}: <a href={v:?}>"));
                    }
                }
                ("img", "src") => {
                    let lower = v.to_ascii_lowercase();
                    if ![
                        "data:image/png",
                        "data:image/gif",
                        "data:image/jpeg",
                        "data:image/webp",
                    ]
                    .iter()
                    .any(|p| lower.starts_with(p))
                    {
                        bad.push(format!("{name}: <img src={v:?}>"));
                    }
                }
                (_, "src") => bad.push(format!("{name}: <{} src>", t.name)),
                ("img", "data-src") => {
                    if !v.starts_with(SCHEME) {
                        bad.push(format!("{name}: <img data-src={v:?}>"));
                    }
                }
                (_, "data-src") => bad.push(format!("{name}: <{} data-src>", t.name)),
                ("input", _) => {
                    let checkbox = t
                        .attrs
                        .iter()
                        .any(|(k, v)| k == "type" && v.eq_ignore_ascii_case("checkbox"));
                    let disabled = t.attrs.iter().any(|(k, _)| k == "disabled");
                    if !checkbox || !disabled {
                        bad.push(format!("{name}: <input {:?}>", t.attrs));
                    }
                }
                _ => {}
            }
        }
    }
    bad
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
