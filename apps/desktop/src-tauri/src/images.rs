//! The `quire-img` URI scheme: images for the Markdown preview (ADR-0028).
//!
//! - `local/<percent-encoded path>?w=<px>`: a local file, served only if its
//!   bytes are an image (PNG, JPEG, GIF, WebP, BMP, ICO, SVG; sniffed, not
//!   trusted from the extension). Raster images wider than `w` device pixels
//!   are decoded and scaled to `w` before they reach the webview (§5.5:
//!   decode at display resolution), and the results are cached in an
//!   `LruCache` registered with the memory governor.
//! - `remote/<doc>/<percent-encoded URL>?w=<px>`: fetched by the host, only
//!   while document `doc` has "load remote content" on (§8); the webview's CSP
//!   never names a remote origin. The response must be an image too.
//!
//! Everything else is 404. Responses carry `nosniff` and a CSP that blocks
//! anything an SVG might try to load.

use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use image::{ImageFormat, ImageReader, Limits};
use parking_lot::Mutex;
use quire_core::memory::{CacheClass, LruCache, MemoryGovernor};
use quire_markdown::images::percent_decode;
use tauri::http::{Request, Response, StatusCode};

/// The scheme name, and the URL prefix the webview uses for it.
pub const SCHEME: &str = "quire-img";

pub fn url_prefix() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://quire-img.localhost/"
    } else {
        "quire-img://localhost/"
    }
}

/// Largest file or download served.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REMOTE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest scaled width (device pixels).
const MAX_WIDTH: u32 = 8192;
/// Decoded-image allocation limit (decompression bombs).
const MAX_ALLOC: u64 = 1024 * 1024 * 1024;
const WORKERS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    source: String,
    /// File modification time and size (local), so an edited image reloads.
    stamp: Option<(SystemTime, u64)>,
    width: u32,
}

#[derive(Clone)]
struct Served {
    bytes: Arc<Vec<u8>>,
    mime: &'static str,
}

type Job = Box<dyn FnOnce() + Send>;

pub struct ImageService {
    cache: Arc<LruCache<Key, Served>>,
    /// Documents with remote content on.
    remote: Mutex<HashSet<u64>>,
    agent: OnceLock<ureq::Agent>,
    jobs: Mutex<Sender<Job>>,
}

impl std::fmt::Debug for ImageService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageService").finish_non_exhaustive()
    }
}

/// What a request failed with.
#[derive(Debug, PartialEq, Eq)]
enum Refused {
    NotFound,
    NotAnImage,
    TooLarge,
    RemoteOff,
    Failed(String),
}

impl Refused {
    fn status(&self) -> StatusCode {
        match self {
            Refused::NotFound => StatusCode::NOT_FOUND,
            Refused::NotAnImage => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Refused::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Refused::RemoteOff => StatusCode::FORBIDDEN,
            Refused::Failed(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

/// The image type of `bytes`, by signature.
fn sniff(bytes: &[u8]) -> Option<(&'static str, Option<ImageFormat>)> {
    let b = bytes;
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", Some(ImageFormat::Png)));
    }
    if b.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(("image/jpeg", Some(ImageFormat::Jpeg)));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(("image/gif", Some(ImageFormat::Gif)));
    }
    if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some(("image/webp", Some(ImageFormat::WebP)));
    }
    if b.starts_with(b"BM") && b.len() > 26 {
        return Some(("image/bmp", Some(ImageFormat::Bmp)));
    }
    if b.starts_with(&[0, 0, 1, 0]) && b.len() > 6 {
        return Some(("image/x-icon", Some(ImageFormat::Ico)));
    }
    // SVG: text whose first element is `<svg` (after an XML declaration,
    // comments, a doctype and whitespace). Served as is: in an <img> it
    // can't run scripts or load anything.
    let head = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    let mut rest = head.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("<?") {
            rest = r.split_once("?>").map_or("", |x| x.1).trim_start();
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->").map_or("", |x| x.1).trim_start();
        } else if rest.starts_with("<!") {
            rest = rest.split_once('>').map_or("", |x| x.1).trim_start();
        } else {
            break;
        }
    }
    if rest.starts_with("<svg") {
        return Some(("image/svg+xml", None));
    }
    None
}

/// Scales a raster image wider than `width` down to it; `None` when the
/// original can be served as is.
fn scaled(bytes: &[u8], format: ImageFormat, width: u32) -> Result<Option<Served>, Refused> {
    if width == 0 {
        return Ok(None);
    }
    let reader = |b: &[u8]| {
        let mut r = ImageReader::with_format(Cursor::new(b.to_vec()), format);
        let mut limits = Limits::default();
        limits.max_alloc = Some(MAX_ALLOC);
        r.limits(limits);
        r
    };
    let (w, h) = reader(bytes)
        .into_dimensions()
        .map_err(|_| Refused::NotAnImage)?;
    // Animated GIFs and WebPs are served as they are (scaling would keep
    // only the first frame).
    if w <= width || matches!(format, ImageFormat::Gif | ImageFormat::WebP) {
        return Ok(None);
    }
    let img = reader(bytes)
        .decode()
        .map_err(|e| Refused::Failed(e.to_string()))?;
    let height = ((h as u64 * width as u64) / w as u64).max(1) as u32;
    let small = img.resize_exact(width, height, image::imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    let (fmt, mime) = if small.color().has_alpha() {
        (ImageFormat::Png, "image/png")
    } else {
        (ImageFormat::Jpeg, "image/jpeg")
    };
    let small = if fmt == ImageFormat::Jpeg {
        image::DynamicImage::ImageRgb8(small.to_rgb8())
    } else {
        small
    };
    small
        .write_to(&mut out, fmt)
        .map_err(|e| Refused::Failed(e.to_string()))?;
    Ok(Some(Served {
        bytes: Arc::new(out.into_inner()),
        mime,
    }))
}

fn width_param(query: Option<&str>) -> u32 {
    query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|kv| kv.strip_prefix("w="))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        .min(MAX_WIDTH)
}

impl ImageService {
    pub fn new(governor: &MemoryGovernor) -> Arc<Self> {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(std::sync::Mutex::new(rx));
        for i in 0..WORKERS {
            let rx: Arc<std::sync::Mutex<Receiver<Job>>> = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("quire-img-{i}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(r) => r.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("spawn image worker");
        }
        Arc::new(Self {
            cache: LruCache::new(governor, "image-scheme", None, CacheClass::DecodedImage),
            remote: Mutex::new(HashSet::new()),
            agent: OnceLock::new(),
            jobs: Mutex::new(tx),
        })
    }

    /// Turns a document's "load remote content" on or off.
    pub fn set_remote(&self, doc: u64, on: bool) {
        let mut r = self.remote.lock();
        if on {
            r.insert(doc);
        } else {
            r.remove(&doc);
        }
    }

    pub fn remote_enabled(&self, doc: u64) -> bool {
        self.remote.lock().contains(&doc)
    }

    /// Handles one request on a worker thread.
    pub fn handle(
        self: &Arc<Self>,
        request: Request<Vec<u8>>,
        respond: impl FnOnce(Response<Vec<u8>>) + Send + 'static,
    ) {
        let me = Arc::clone(self);
        let job: Job = Box::new(move || {
            let uri = request.uri();
            let path = uri.path().trim_start_matches('/').to_owned();
            let width = width_param(uri.query());
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| me.serve(&path, width)))
                    .unwrap_or_else(|_| Err(Refused::Failed("panic".into())));
            respond(response(result));
        });
        if let Err(e) = self.jobs.lock().send(job) {
            // The workers are gone (shutting down).
            (e.0)();
        }
    }

    fn serve(&self, path: &str, width: u32) -> Result<Served, Refused> {
        if let Some(enc) = path.strip_prefix("local/") {
            let p = percent_decode(enc).ok_or(Refused::NotFound)?;
            return self.local(PathBuf::from(p), width);
        }
        if let Some(rest) = path.strip_prefix("remote/") {
            let (doc, enc) = rest.split_once('/').ok_or(Refused::NotFound)?;
            let doc: u64 = doc.parse().map_err(|_| Refused::NotFound)?;
            if !self.remote_enabled(doc) {
                return Err(Refused::RemoteOff);
            }
            let url = percent_decode(enc).ok_or(Refused::NotFound)?;
            return self.remote(&url, width);
        }
        Err(Refused::NotFound)
    }

    fn local(&self, path: PathBuf, width: u32) -> Result<Served, Refused> {
        let meta = std::fs::metadata(&path).map_err(|_| Refused::NotFound)?;
        if !meta.is_file() {
            return Err(Refused::NotFound);
        }
        if meta.len() > MAX_BYTES {
            return Err(Refused::TooLarge);
        }
        let key = Key {
            source: path.to_string_lossy().into_owned(),
            stamp: Some((
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
            )),
            width,
        };
        if let Some(s) = self.cache.get(&key) {
            return Ok(s);
        }
        // Check the signature before reading the whole file.
        let mut f = std::fs::File::open(&path).map_err(|_| Refused::NotFound)?;
        let mut head = vec![0u8; 4096];
        let n = read_up_to(&mut f, &mut head).map_err(|_| Refused::NotFound)?;
        head.truncate(n);
        sniff(&head).ok_or(Refused::NotAnImage)?;
        let mut bytes = head;
        f.read_to_end(&mut bytes).map_err(|_| Refused::NotFound)?;
        self.finish(key, bytes, width)
    }

    fn finish(&self, key: Key, bytes: Vec<u8>, width: u32) -> Result<Served, Refused> {
        let (mime, format) = sniff(&bytes).ok_or(Refused::NotAnImage)?;
        let served = match format.map(|f| scaled(&bytes, f, width)).transpose()? {
            Some(Some(s)) => s,
            _ => Served {
                bytes: Arc::new(bytes),
                mime,
            },
        };
        self.cache
            .insert(key, served.clone(), served.bytes.len() as u64);
        Ok(served)
    }

    fn remote(&self, url: &str, width: u32) -> Result<Served, Refused> {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(Refused::NotFound);
        }
        let key = Key {
            source: url.to_owned(),
            stamp: None,
            width,
        };
        if let Some(s) = self.cache.get(&key) {
            return Ok(s);
        }
        let agent = self.agent.get_or_init(|| {
            let tls = ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build();
            ureq::Agent::config_builder()
                .tls_config(tls)
                .timeout_global(Some(Duration::from_secs(20)))
                .max_redirects(5)
                .user_agent("quire")
                .build()
                .new_agent()
        });
        let mut resp = agent
            .get(url)
            .call()
            .map_err(|e| Refused::Failed(e.to_string()))?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_REMOTE_BYTES)
            .read_to_vec()
            .map_err(|e| Refused::Failed(e.to_string()))?;
        self.finish(key, bytes, width)
    }
}

fn read_up_to(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn response(result: Result<Served, Refused>) -> Response<Vec<u8>> {
    let builder = Response::builder()
        .header("X-Content-Type-Options", "nosniff")
        .header(
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'",
        )
        .header("Cache-Control", "no-store");
    let r = match result {
        Ok(s) => builder
            .status(StatusCode::OK)
            .header("Content-Type", s.mime)
            .body(s.bytes.as_ref().clone()),
        Err(e) => {
            if let Refused::Failed(msg) = &e {
                tracing::debug!(%msg, "preview image failed");
            }
            builder
                .status(e.status())
                .header("Content-Type", "text/plain")
                .body(Vec::new())
        }
    };
    r.unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::memory::GovernorConfig;
    use quire_markdown::images::percent_encode;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 128]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn service() -> Arc<ImageService> {
        ImageService::new(&MemoryGovernor::new(GovernorConfig::default()))
    }

    #[test]
    fn sniffs_images_not_extensions() {
        assert_eq!(sniff(&png(1, 1)).map(|s| s.0), Some("image/png"));
        assert_eq!(
            sniff(b"<?xml version=\"1.0\"?>\n<!-- c --><svg xmlns=\"\"/>").map(|s| s.0),
            Some("image/svg+xml")
        );
        assert_eq!(sniff(b"[boot loader]\r\ntimeout=30"), None);
        assert_eq!(sniff(b"<html><svg>"), None);
    }

    #[test]
    fn serves_local_images_scaled_and_refuses_other_files() {
        let dir = std::env::temp_dir().join(format!("quire-img-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("big.png");
        std::fs::write(&img, png(400, 200)).unwrap();
        let txt = dir.join("secret.png");
        std::fs::write(&txt, b"not an image").unwrap();
        let s = service();
        let enc = |p: &std::path::Path| format!("local/{}", percent_encode(&p.to_string_lossy()));

        let full = s.serve(&enc(&img), 0).unwrap();
        assert_eq!(full.mime, "image/png");
        let small = s.serve(&enc(&img), 100).unwrap();
        let dims = ImageReader::new(Cursor::new(small.bytes.as_ref().clone()))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert_eq!(dims, (100, 50));
        assert!(s.cache.len() >= 2);

        assert_eq!(s.serve(&enc(&txt), 0).err(), Some(Refused::NotAnImage));
        assert_eq!(
            s.serve(&enc(&dir.join("none.png")), 0).err(),
            Some(Refused::NotFound)
        );
        assert_eq!(s.serve(&enc(&dir), 0).err(), Some(Refused::NotFound));
        assert_eq!(s.serve("other/x", 0).err(), Some(Refused::NotFound));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_needs_the_documents_toggle() {
        let s = service();
        let path = format!(
            "remote/3/{}",
            percent_encode("https://example.invalid/a.png")
        );
        assert_eq!(s.serve(&path, 0).err(), Some(Refused::RemoteOff));
        s.set_remote(3, true);
        assert!(s.remote_enabled(3));
        // Non-HTTP URLs are refused even with the toggle on.
        let file = format!("remote/3/{}", percent_encode("file:///C:/x.png"));
        assert_eq!(s.serve(&file, 0).err(), Some(Refused::NotFound));
        s.set_remote(3, false);
        assert_eq!(s.serve(&path, 0).err(), Some(Refused::RemoteOff));
    }

    #[test]
    fn width_parameter() {
        assert_eq!(width_param(Some("w=320")), 320);
        assert_eq!(width_param(Some("x=1&w=99999")), MAX_WIDTH);
        assert_eq!(width_param(None), 0);
    }
}
