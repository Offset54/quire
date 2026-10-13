//! HTML normalization for spec comparisons, after the CommonMark spec's
//! `test/normalize.py` (which adapts karlcow/markdown-testsuite): whitespace
//! collapsed outside `<pre>`, whitespace around block tags removed,
//! self-closing tags as open tags, attributes sorted, character references
//! decoded except `< > & "`. Both sides of a comparison go through it.

const BLOCK_TAGS: &[&str] = &[
    "article",
    "header",
    "aside",
    "hgroup",
    "blockquote",
    "hr",
    "iframe",
    "body",
    "li",
    "map",
    "button",
    "object",
    "canvas",
    "ol",
    "caption",
    "output",
    "col",
    "p",
    "colgroup",
    "pre",
    "dd",
    "progress",
    "div",
    "section",
    "dl",
    "table",
    "td",
    "dt",
    "tbody",
    "embed",
    "textarea",
    "fieldset",
    "tfoot",
    "figcaption",
    "th",
    "figure",
    "thead",
    "footer",
    "tr",
    "form",
    "ul",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "video",
    "script",
    "style",
];

#[derive(PartialEq, Clone, Copy)]
enum Last {
    StartTag,
    EndTag,
    Data,
    Other,
}

struct Norm {
    out: String,
    last: Last,
    last_tag: String,
    in_pre: bool,
}

fn is_block(tag: &str) -> bool {
    BLOCK_TAGS.contains(&tag)
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut ws = false;
    for c in s.chars() {
        // Python's `\s` on str: Unicode whitespace.
        if c.is_whitespace() {
            if !ws {
                out.push(' ');
            }
            ws = true;
        } else {
            out.push(c);
            ws = false;
        }
    }
    out
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
}

/// Decodes a reference body (`amp`, `#123`, `#x1F`); `None` if unknown.
fn decode_ref(body: &str) -> Option<char> {
    if let Some(num) = body.strip_prefix('#') {
        let v = if let Some(hex) = num.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse::<u32>().ok()?
        };
        return char::from_u32(v);
    }
    Some(match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "nbsp" => '\u{a0}',
        "copy" => '\u{a9}',
        _ => return None,
    })
}

/// Decodes references in an attribute value (as `html.parser` does).
fn unescape_attr(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        match tail
            .find(';')
            .filter(|&j| j <= 32)
            .and_then(|j| decode_ref(&tail[..j]).map(|c| (j, c)))
        {
            Some((j, c)) => {
                out.push(c);
                rest = &tail[j + 1..];
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

impl Norm {
    fn data(&mut self, data: &str) {
        let after_tag = matches!(self.last, Last::EndTag | Last::StartTag);
        let after_block = after_tag && is_block(&self.last_tag);
        let mut d = data.to_string();
        if after_tag && self.last_tag == "br" {
            d = d.trim_start_matches('\n').to_string();
        }
        if !self.in_pre {
            d = collapse_ws(&d);
        }
        if after_block && !self.in_pre {
            if self.last == Last::StartTag {
                d = d.trim_start().to_string();
            } else {
                d = d.trim().to_string();
            }
        }
        // Unlike normalize.py, `"` and `>` in text compare equal to their
        // references: both serializations are the same HTML.
        for c in d.chars() {
            match c {
                '"' => self.out.push_str("&quot;"),
                '>' => self.out.push_str("&gt;"),
                c => self.out.push(c),
            }
        }
        self.last = Last::Data;
    }

    fn text(&mut self, text: &str) {
        // Split into data and character references.
        let mut rest = text;
        while let Some(i) = rest.find('&') {
            if i > 0 {
                self.data(&rest[..i]);
            }
            let tail = &rest[i + 1..];
            let end = tail.find(';').filter(|&j| {
                j > 0
                    && j <= 32
                    && tail[..j]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '#')
            });
            match end {
                Some(j) => {
                    match decode_ref(&tail[..j]) {
                        Some(c) => escape_char(c, &mut self.out),
                        None => {
                            self.out.push('&');
                            self.out.push_str(&tail[..=j]);
                        }
                    }
                    self.last = Last::Other;
                    rest = &tail[j + 1..];
                }
                None => {
                    self.data("&");
                    rest = tail;
                }
            }
        }
        if !rest.is_empty() {
            self.data(rest);
        }
    }

    fn tag(&mut self, chunk: &str) -> bool {
        let inner = &chunk[1..chunk.len() - 1];
        if let Some(c) = inner.strip_prefix("!--") {
            let c = c.strip_suffix("--").unwrap_or(c);
            self.out.push_str("<!--");
            self.out.push_str(c);
            self.out.push_str("-->");
            self.last = Last::Other;
            return true;
        }
        if inner.starts_with('!') || inner.starts_with('?') {
            self.out.push_str(chunk);
            self.last = Last::Other;
            return true;
        }
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim_end();
            if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
                return false;
            }
            let tag = name
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if tag == "pre" {
                self.in_pre = false;
            } else if is_block(&tag) {
                let t = self.out.trim_end().len();
                self.out.truncate(t);
            }
            self.out.push_str("</");
            self.out.push_str(&tag);
            self.out.push('>');
            self.last_tag = tag;
            self.last = Last::EndTag;
            return true;
        }
        if !inner.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return false;
        }
        let (body, self_closing) = match inner.strip_suffix('/') {
            Some(b) => (b, true),
            None => (inner, false),
        };
        let name_end = body
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(body.len());
        let tag = body[..name_end].to_ascii_lowercase();
        let Some(mut attrs) = parse_attrs(&body[name_end..]) else {
            return false;
        };
        if tag == "pre" {
            self.in_pre = true;
        }
        if is_block(&tag) {
            let t = self.out.trim_end().len();
            self.out.truncate(t);
        }
        self.out.push('<');
        self.out.push_str(&tag);
        attrs.sort();
        for (k, v) in attrs {
            self.out.push(' ');
            self.out.push_str(&k);
            if let Some(v) = v {
                self.out.push_str("=\"");
                escape(&v, &mut self.out);
                self.out.push('"');
            }
        }
        self.out.push('>');
        self.last_tag = tag;
        self.last = if self_closing {
            Last::EndTag
        } else {
            Last::StartTag
        };
        true
    }
}

fn escape_char(c: char, out: &mut String) {
    match c {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        '"' => out.push_str("&quot;"),
        c => out.push(c),
    }
}

type Attrs = Vec<(String, Option<String>)>;

fn parse_attrs(mut s: &str) -> Option<Attrs> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if s.is_empty() {
            return Some(out);
        }
        let end = s
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')
            .unwrap_or(s.len());
        if end == 0 {
            return None;
        }
        let name = s[..end].to_ascii_lowercase();
        s = s[end..].trim_start();
        if let Some(v) = s.strip_prefix('=') {
            let v = v.trim_start();
            let (val, rest) = match v.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let close = v[1..].find(q)?;
                    (&v[1..=close], &v[close + 2..])
                }
                _ => {
                    let e = v.find(char::is_whitespace).unwrap_or(v.len());
                    (&v[..e], &v[e..])
                }
            };
            out.push((name, Some(unescape_attr(val))));
            s = rest;
        } else {
            out.push((name, None));
        }
    }
}

pub fn normalize(html: &str) -> String {
    let mut n = Norm {
        out: String::new(),
        last: Last::StartTag,
        last_tag: String::new(),
        in_pre: false,
    };
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").map_or(rest.len(), |i| i + 12);
            n.out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if rest.starts_with('<') {
            match rest.find('>') {
                Some(i) => {
                    let chunk = &rest[..=i];
                    if !n.tag(chunk) {
                        n.text(chunk);
                    }
                    rest = &rest[i + 1..];
                }
                None => {
                    n.text(rest);
                    rest = "";
                }
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            n.text(&rest[..end]);
            rest = &rest[end..];
        }
    }
    n.out
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn doc_examples() {
        assert_eq!(normalize("<p>a  \t b</p>"), "<p>a b</p>");
        assert_eq!(normalize("<p>a  \t\nb</p>"), "<p>a b</p>");
        assert_eq!(normalize(" <p>a  b</p>"), "<p>a b</p>");
        assert_eq!(normalize("\n\t<p>\n\t\ta  b\t\t</p>\n\t"), "<p>a b</p>");
        assert_eq!(normalize("<i>a  b</i> "), "<i>a b</i> ");
        assert_eq!(normalize("<br />"), "<br>");
        assert_eq!(
            normalize("<a title=\"bar\" HREF=\"foo\">x</a>"),
            "<a href=\"foo\" title=\"bar\">x</a>"
        );
        assert_eq!(normalize("&amp;&gt;&lt;&quot;"), "&amp;&gt;&lt;&quot;");
    }
}
