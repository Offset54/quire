//! Where an image in the preview comes from (§6.2, §8; ADR-0028).
//!
//! Every image source, from Markdown (`![a](src)`) or raw HTML (`<img src>`),
//! goes through [`ImagePolicy::resolve`]:
//!
//! - `data:` images (PNG, GIF, JPEG, WebP) stay inline.
//! - Local images (relative paths, absolute paths, `file:` URLs: any readable
//!   path, user decision B) become URLs of the host's image scheme, which
//!   serves only image files, decoded at display size. They load lazily
//!   (`data-src`): the preview sets `src` with the display width when the
//!   image comes near the viewport.
//! - Remote images (`http:`, `https:`, `//host`) are placeholders naming the
//!   source until the document's "load remote content" toggle is on; then the
//!   host fetches them through the same scheme, so the webview's CSP never
//!   allows a remote origin.
//! - Anything else (other schemes) is a placeholder.

use std::path::{Path, PathBuf};

/// The default scheme prefix (Windows' form of `quire-img://localhost/`,
/// which WebView2 serves as `http://quire-img.localhost/`).
pub const DEFAULT_SCHEME: &str = "http://quire-img.localhost/";

/// What to do with an image source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSrc {
    /// Load it now (inline data).
    Url(String),
    /// Load it from our scheme once near the viewport (`data-src`).
    Lazy(String),
    /// Blocked or unresolvable: a placeholder naming the source
    /// (`data-blocked-src`).
    Placeholder(String),
}

/// A document's image policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePolicy {
    /// Folder relative paths resolve against (the document's). `None` for an
    /// unsaved document: relative images are placeholders.
    pub base_dir: Option<PathBuf>,
    /// Prefix of the host's image scheme URLs, ending in `/`.
    pub scheme: String,
    /// The document key the host checks the remote toggle against, when the
    /// toggle is on. `None`: remote images are placeholders.
    pub remote: Option<u64>,
}

impl Default for ImagePolicy {
    fn default() -> Self {
        Self {
            base_dir: None,
            scheme: DEFAULT_SCHEME.to_owned(),
            remote: None,
        }
    }
}

/// Percent-encodes everything but unreserved characters (RFC 3986).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes `%XX` escapes (invalid ones stay as written); `None` if the result
/// isn't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The scheme of a URL, lowercased, if it has one (a drive letter like `C:`
/// is not a scheme).
fn scheme_of(url: &str) -> Option<String> {
    let i = url.find(':')?;
    let s = &url[..i];
    let ok = s.len() > 1
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'));
    ok.then(|| s.to_ascii_lowercase())
}

/// `data:` images comrak and browsers treat as safe.
fn safe_data_image(url: &str) -> bool {
    let lower = url.get(..16).unwrap_or(url).to_ascii_lowercase();
    [
        "data:image/png",
        "data:image/gif",
        "data:image/jpeg",
        "data:image/webp",
    ]
    .iter()
    .any(|p| lower.starts_with(p))
}

/// A local path from a `file:` URL (`file:///C:/x`, `file://server/share/x`,
/// `file:/x`).
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = &url[5..];
    let path = if let Some(r) = rest.strip_prefix("///") {
        // `file:///C:/x` on Windows, `file:///x` elsewhere.
        if r.len() >= 2 && r.as_bytes()[1] == b':' {
            r.to_owned()
        } else {
            format!("/{r}")
        }
    } else if let Some(r) = rest.strip_prefix("//") {
        // A UNC host (`localhost` means none).
        match r.strip_prefix("localhost/") {
            Some(p) => format!("/{p}"),
            None => format!("//{r}"),
        }
    } else {
        rest.to_owned()
    };
    let path = strip_query(&path);
    percent_decode(path).map(PathBuf::from)
}

fn strip_query(s: &str) -> &str {
    let end = s.find(['?', '#']).unwrap_or(s.len());
    &s[..end]
}

impl ImagePolicy {
    /// Decides what an image source becomes.
    pub fn resolve(&self, src: &str) -> ImageSrc {
        let url = src.trim();
        if url.is_empty() {
            return ImageSrc::Placeholder(String::new());
        }
        let remote = url.starts_with("//") && !url.starts_with("///");
        match scheme_of(url).as_deref() {
            Some("data") => {
                if safe_data_image(url) {
                    ImageSrc::Url(url.to_owned())
                } else {
                    ImageSrc::Placeholder(url.chars().take(64).collect())
                }
            }
            Some("http" | "https") => self.remote_src(url),
            _ if remote => self.remote_src(&format!("https:{url}")),
            Some("file") => match file_url_path(url) {
                Some(p) => self.local_src(&p),
                None => ImageSrc::Placeholder(url.to_owned()),
            },
            Some(_) => ImageSrc::Placeholder(url.to_owned()),
            None => {
                let Some(path) = percent_decode(strip_query(url)) else {
                    return ImageSrc::Placeholder(url.to_owned());
                };
                let p = Path::new(&path);
                if is_absolute(&path) {
                    self.local_src(p)
                } else {
                    match &self.base_dir {
                        Some(base) => self.local_src(&base.join(p)),
                        None => ImageSrc::Placeholder(url.to_owned()),
                    }
                }
            }
        }
    }

    fn local_src(&self, path: &Path) -> ImageSrc {
        let path = path.to_string_lossy();
        ImageSrc::Lazy(format!("{}local/{}", self.scheme, percent_encode(&path)))
    }

    fn remote_src(&self, url: &str) -> ImageSrc {
        match self.remote {
            Some(doc) => ImageSrc::Lazy(format!(
                "{}remote/{doc}/{}",
                self.scheme,
                percent_encode(url)
            )),
            None => ImageSrc::Placeholder(url.to_owned()),
        }
    }
}

/// An absolute path on any platform we ship on (`/x`, `C:\x`, `C:/x`, `\\server\x`).
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    b.first() == Some(&b'/')
        || b.first() == Some(&b'\\')
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(remote: bool) -> ImagePolicy {
        ImagePolicy {
            base_dir: Some(PathBuf::from("/docs/notes")),
            scheme: "s:/".into(),
            remote: remote.then_some(7),
        }
    }

    #[test]
    fn local_paths_go_through_the_scheme() {
        let p = policy(false);
        let joined = Path::new("/docs/notes").join("img/a b.png");
        assert_eq!(
            p.resolve("img/a%20b.png?x#y"),
            ImageSrc::Lazy(format!(
                "s:/local/{}",
                percent_encode(&joined.to_string_lossy())
            ))
        );
        assert_eq!(
            p.resolve("C:\\pics\\x.png"),
            ImageSrc::Lazy("s:/local/C%3A%5Cpics%5Cx.png".into())
        );
        assert_eq!(
            p.resolve("file:///C:/pics/x.png"),
            ImageSrc::Lazy("s:/local/C%3A%2Fpics%2Fx.png".into())
        );
        assert_eq!(
            p.resolve("file:///home/x.png"),
            ImageSrc::Lazy("s:/local/%2Fhome%2Fx.png".into())
        );
        let unsaved = ImagePolicy::default();
        assert_eq!(
            unsaved.resolve("a.png"),
            ImageSrc::Placeholder("a.png".into())
        );
    }

    #[test]
    fn remote_images_wait_for_the_toggle() {
        assert_eq!(
            policy(false).resolve("https://e.com/a.png"),
            ImageSrc::Placeholder("https://e.com/a.png".into())
        );
        assert_eq!(
            policy(true).resolve("https://e.com/a.png"),
            ImageSrc::Lazy("s:/remote/7/https%3A%2F%2Fe.com%2Fa.png".into())
        );
        assert_eq!(
            policy(false).resolve("//e.com/a.png"),
            ImageSrc::Placeholder("https://e.com/a.png".into())
        );
    }

    #[test]
    fn data_and_other_schemes() {
        let p = policy(true);
        assert_eq!(
            p.resolve("data:image/png;base64,AA"),
            ImageSrc::Url("data:image/png;base64,AA".into())
        );
        assert!(matches!(
            p.resolve("data:text/html,<script>"),
            ImageSrc::Placeholder(_)
        ));
        assert!(matches!(
            p.resolve("javascript:alert(1)"),
            ImageSrc::Placeholder(_)
        ));
        assert!(matches!(
            p.resolve("ftp://e.com/x.png"),
            ImageSrc::Placeholder(_)
        ));
        assert_eq!(p.resolve(" "), ImageSrc::Placeholder(String::new()));
    }

    #[test]
    fn percent_round_trip() {
        let s = "C:\\Ünï code\\a b%.png";
        assert_eq!(percent_decode(&percent_encode(s)).as_deref(), Some(s));
        assert_eq!(percent_decode("%zz%4").as_deref(), Some("%zz%4"));
    }
}
