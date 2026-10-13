//! Allowlist sanitizer for raw HTML in Markdown (§6.2, §8; ADR-0028).
//!
//! comrak escapes all text and builds the markup for Markdown constructs
//! itself, so only raw HTML (HTML blocks and inline HTML) needs cleaning. Each
//! fragment is tokenized; allowed tags keep allowed attributes only, other
//! tags are dropped (their text stays), and elements whose content is code or
//! a document of its own (`script`, `style`, `iframe`, …) are dropped with
//! their content. Comments, doctypes and processing instructions go. URLs in
//! `href` keep only safe schemes; image sources go through the caller's
//! policy (local images through our scheme, remote ones behind the
//! per-document toggle).

use std::fmt::Write as _;

/// Tags kept (GitHub's list, without forms and media).
const ALLOWED_TAGS: &[&str] = &[
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

/// Tags dropped together with everything up to their end tag.
const DROP_WITH_CONTENT: &[&str] = &[
    "script",
    "style",
    "iframe",
    "object",
    "embed",
    "noscript",
    "template",
    "textarea",
    "title",
    "xmp",
    "noembed",
    "noframes",
    "plaintext",
    "frameset",
    "frame",
    "applet",
    "svg",
    "math",
    "select",
    "audio",
    "video",
    "canvas",
];

/// Attributes kept on any allowed tag.
const GLOBAL_ATTRS: &[&str] = &[
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
];

/// What to do with an image source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSrc {
    /// Use this URL (already rewritten, e.g. to our image scheme).
    Url(String),
    /// Blocked (remote content not enabled): show a placeholder naming it.
    Placeholder(String),
}

/// Schemes an `href` may use; relative and fragment links are fine too.
fn safe_href(url: &str) -> bool {
    let u = url.trim();
    let lower: String = u
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    match lower.find(':') {
        None => true,
        Some(i) => {
            // A colon after a `/`, `?` or `#` isn't a scheme.
            if lower[..i].contains(['/', '?', '#']) {
                return true;
            }
            matches!(&lower[..i], "http" | "https" | "mailto")
        }
    }
}

struct Tag {
    name: String,
    end: bool,
    attrs: Vec<(String, Option<String>)>,
    self_closing: bool,
}

/// Decodes character references in an attribute value (enough to see what a
/// URL really is: `&#106;avascript:` is `javascript:`).
fn decode_attr(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').filter(|&e| e > 0 && e <= 12);
        let decoded = end.and_then(|e| {
            let body = &tail[..e];
            let c = if let Some(n) = body.strip_prefix('#') {
                if let Some(h) = n.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(h, 16).ok().and_then(char::from_u32)
                } else {
                    n.parse::<u32>().ok().and_then(char::from_u32)
                }
            } else {
                match body {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "colon" => Some(':'),
                    "tab" => Some('\t'),
                    "newline" => Some('\n'),
                    _ => None,
                }
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses a tag starting at `s[0] == '<'`; returns it and its length.
fn parse_tag(s: &str) -> Option<(Tag, usize)> {
    let b = s.as_bytes();
    let mut i = 1;
    let end = b.get(1) == Some(&b'/');
    if end {
        i += 1;
    }
    let name_start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
        i += 1;
    }
    if i == name_start || !b[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = s[name_start..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        match b.get(i) {
            None => return None,
            Some(b'>') => {
                i += 1;
                break;
            }
            Some(b'/') if b.get(i + 1) == Some(&b'>') => {
                self_closing = true;
                i += 2;
                break;
            }
            Some(b'/') => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let a = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if i == a {
            i += 1;
            continue;
        }
        let aname = s[a..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = None;
        if b.get(i) == Some(&b'=') {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            match b.get(i) {
                Some(&q @ (b'"' | b'\'')) => {
                    let close = s[i + 1..].find(q as char)?;
                    value = Some(decode_attr(&s[i + 1..i + 1 + close]));
                    i += close + 2;
                }
                _ => {
                    let v = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                        i += 1;
                    }
                    value = Some(decode_attr(&s[v..i]));
                }
            }
        }
        attrs.push((aname, value));
    }
    Some((
        Tag {
            name,
            end,
            attrs,
            self_closing,
        },
        i,
    ))
}

fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Cleans one fragment of raw HTML. `image` decides each `<img src>`.
/// `skip` carries an open drop-with-content element across fragments of one
/// block (an HTML block can end inside one).
pub fn sanitize(
    html: &str,
    image: &mut dyn FnMut(&str) -> ImageSrc,
    skip: &mut Option<String>,
) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if skip.is_none() {
                escape_text(rest, &mut out);
            }
            break;
        };
        if skip.is_none() {
            escape_text(&rest[..lt], &mut out);
        }
        rest = &rest[lt..];
        // Comments, doctypes, CDATA and processing instructions: dropped.
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        let Some((tag, len)) = parse_tag(rest) else {
            if skip.is_none() {
                out.push_str("&lt;");
            }
            rest = &rest[1..];
            continue;
        };
        rest = &rest[len..];
        if let Some(open) = skip.as_deref() {
            if tag.end && tag.name == open {
                *skip = None;
            }
            continue;
        }
        if DROP_WITH_CONTENT.contains(&tag.name.as_str()) {
            if !tag.end && !tag.self_closing {
                *skip = Some(tag.name);
            }
            continue;
        }
        if !ALLOWED_TAGS.contains(&tag.name.as_str()) {
            continue;
        }
        if tag.end {
            let _ = write!(out, "</{}>", tag.name);
            continue;
        }
        // A checkbox (task lists written as HTML) is the only input kept.
        if tag.name == "input"
            && !tag.attrs.iter().any(|(k, v)| {
                k == "type"
                    && v.as_deref()
                        .is_some_and(|v| v.eq_ignore_ascii_case("checkbox"))
            })
        {
            continue;
        }
        out.push('<');
        out.push_str(&tag.name);
        for (k, v) in &tag.attrs {
            let keep = match (tag.name.as_str(), k.as_str()) {
                ("a", "href") => v.as_deref().is_some_and(safe_href),
                ("img", "src") => {
                    if let Some(v) = v {
                        match image(v) {
                            ImageSrc::Url(u) => {
                                out.push_str(" src=\"");
                                escape_attr(&u, &mut out);
                                out.push('"');
                            }
                            ImageSrc::Placeholder(u) => {
                                out.push_str(" data-blocked-src=\"");
                                escape_attr(&u, &mut out);
                                out.push('"');
                            }
                        }
                    }
                    false
                }
                ("input", "checked") => true,
                (_, k) => GLOBAL_ATTRS.contains(&k),
            };
            if keep {
                out.push(' ');
                out.push_str(k);
                if let Some(v) = v {
                    out.push_str("=\"");
                    escape_attr(v, &mut out);
                    out.push('"');
                }
            }
        }
        if tag.name == "input" {
            out.push_str(" disabled");
        }
        if tag.name == "a" {
            // Links never open inside the app; the frontend handles clicks.
            out.push_str(" rel=\"noopener noreferrer\"");
        }
        out.push_str(if tag.self_closing { " />" } else { ">" });
    }
    out
}

#[cfg(test)]
mod tests;
