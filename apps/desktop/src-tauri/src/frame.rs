//! The `quire-frame` URI scheme: the Mermaid frame page (ADR-0033).
//!
//! Mermaid renders in a sandboxed iframe (opaque origin, no IPC). It needs
//! inline styles to measure its text, which the app's CSP forbids, so the
//! frame is served here, from the app's embedded assets, with a policy of
//! its own: scripts and styles from this scheme only (plus inline styles),
//! images from `data:`, nothing else, and only the app may frame it. Only
//! the frame page and the built assets are served.

use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, Runtime};

pub const SCHEME: &str = "quire-frame";
/// The frame page.
pub const PAGE: &str = "mermaid-frame.html";

pub fn url_prefix() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://quire-frame.localhost/"
    } else {
        "quire-frame://localhost/"
    }
}

const CSP: &str = "default-src 'none'; \
     script-src http://quire-frame.localhost quire-frame:; \
     style-src http://quire-frame.localhost quire-frame: 'unsafe-inline'; \
     font-src http://quire-frame.localhost quire-frame:; \
     img-src data:; \
     base-uri 'none'; form-action 'none'; \
     frame-ancestors http://tauri.localhost tauri://localhost http://localhost:5173";

/// Whether `path` (without the leading slash) may be served.
fn allowed(path: &str) -> bool {
    path == PAGE
        || (path.starts_with("assets/")
            && !path.contains("..")
            && !path.contains('\\')
            && path[7..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path().trim_start_matches('/');
    let builder = Response::builder()
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Security-Policy", CSP)
        // The sandboxed page has an opaque origin: its module scripts are
        // cross-origin requests.
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store");
    let asset = allowed(path)
        .then(|| app.asset_resolver().get(path.to_owned()))
        .flatten();
    let r = match asset {
        Some(a) => {
            let bytes = if path == PAGE {
                without_csp_meta(&a.bytes)
            } else {
                a.bytes
            };
            builder
                .status(StatusCode::OK)
                .header("Content-Type", a.mime_type)
                .body(bytes)
        }
        None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
    };
    r.unwrap_or_else(|_| Response::new(Vec::new()))
}

/// The page without the `<meta http-equiv="Content-Security-Policy">` that
/// Tauri's build adds to every HTML asset: with the app's policy inside the
/// sandboxed frame, its scripts (`'self'` of an opaque origin) and Mermaid's
/// inline styles are blocked. The frame's policy is the header above.
fn without_csp_meta(html: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(html);
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let end = start + len + 1;
        if lower[start..end].contains("content-security-policy") {
            out.push_str(&s[last..start]);
            last = end;
        }
        from = end;
    }
    out.push_str(&s[last..]);
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_the_page_and_built_assets() {
        assert!(allowed("mermaid-frame.html"));
        assert!(allowed("assets/mermaid-frame-Ab12_x.js"));
        assert!(!allowed("index.html"));
        assert!(!allowed("assets/../index.html"));
        assert!(!allowed("assets/a/b.js"));
        assert!(!allowed("assets/a\\b.js"));
        assert!(!allowed(""));
    }

    #[test]
    fn the_app_policy_is_taken_out_of_the_page() {
        let page = br#"<head><meta charset="UTF-8" /><META HTTP-EQUIV="Content-Security-Policy" content="script-src 'self'"><title>x</title></head>"#;
        assert_eq!(
            String::from_utf8(without_csp_meta(page)).unwrap(),
            r#"<head><meta charset="UTF-8" /><title>x</title></head>"#
        );
    }
}
