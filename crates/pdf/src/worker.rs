//! The worker process (ADR-0036): one PDFium instance, one thread, requests
//! from stdin answered in order on stdout. A crash, a hang or a panic here
//! costs the host a restart and a re-queued request, nothing more.

use std::collections::HashMap;
use std::ffi::{CString, c_int, c_ulong, c_void};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::source::{self, Source};

use crate::ffi::{self, Api, FileAccess, FsMatrix, FsRectF, FsSizeF, Handle};
use crate::protocol::{self, Body, MAX_FRAME, Matrix, PROTOCOL_VERSION, Request, Response, errors};

/// Environment variable naming the PDFium library to load.
pub const LIBRARY_ENV: &str = "QUIRE_PDFIUM";
/// Environment variable that enables the `Crash` request (tests only).
pub const FAULTS_ENV: &str = "QUIRE_PDF_FAULTS";

/// Loaded pages kept per document: tiles of the same page reuse the parsed
/// page instead of loading it again.
const PAGE_CACHE: usize = 4;

/// What PDFium reads: the file, then the overlay (unsaved updates).
struct DocBytes {
    file: Option<Arc<dyn Source>>,
    file_len: u64,
    overlay: Vec<u8>,
}

impl DocBytes {
    fn len(&self) -> u64 {
        self.file_len + self.overlay.len() as u64
    }

    fn read(&self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = pos
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= self.len())
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let split = self.file_len.clamp(pos, end);
        let (head, tail) = buf.split_at_mut((split - pos) as usize);
        if !head.is_empty() {
            match &self.file {
                Some(f) => f.read_at(pos, head)?,
                None => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            }
        }
        if !tail.is_empty() {
            let o = (split - self.file_len) as usize;
            tail.copy_from_slice(&self.overlay[o..o + tail.len()]);
        }
        Ok(())
    }
}

/// `FPDF_FILEACCESS::m_GetBlock`. An I/O error (a guarded read of a mapped
/// file that failed, ADR-0007) reaches PDFium as a failed read.
unsafe extern "C" fn get_block(
    param: *mut c_void,
    pos: c_ulong,
    buf: *mut u8,
    size: c_ulong,
) -> c_int {
    // SAFETY: `param` is the `DocBytes` boxed in the `Doc` that owns this
    // `FileAccess`, alive as long as PDFium holds the document; PDFium
    // passes a buffer of `size` writable bytes.
    let (bytes, out) = unsafe {
        (
            &*(param as *const DocBytes),
            std::slice::from_raw_parts_mut(buf, size as usize),
        )
    };
    c_int::from(bytes.read(u64::from(pos), out).is_ok())
}

struct Doc {
    handle: Handle,
    // Kept alive while PDFium may read through them.
    _bytes: Box<DocBytes>,
    _access: Box<FileAccess>,
    /// Most recently used last.
    pages: Vec<(u32, Handle)>,
}

struct Worker {
    api: Api,
    library: PathBuf,
    docs: HashMap<u64, Doc>,
    governor: MemoryGovernor,
    faults: bool,
}

fn error(code: u32, message: impl Into<String>) -> Body {
    Body::Error {
        code,
        message: message.into(),
    }
}

impl Worker {
    fn handle(&mut self, req: Request) -> Body {
        match req {
            Request::Hello { protocol } if protocol == PROTOCOL_VERSION => Body::Hello {
                protocol: PROTOCOL_VERSION,
                pdfium: self.library.display().to_string(),
            },
            Request::Hello { protocol } => error(
                errors::BAD_REQUEST,
                format!("protocol {protocol}, worker speaks {PROTOCOL_VERSION}"),
            ),
            Request::Open {
                doc,
                path,
                password,
                overlay,
            } => {
                self.close(doc);
                self.open(doc, &path, password.as_deref(), overlay)
            }
            Request::Close { doc } => {
                self.close(doc);
                Body::Closed
            }
            Request::PageSizes { doc, from, to } => self.page_sizes(doc, from, to),
            Request::Render {
                doc,
                page,
                matrix,
                width,
                height,
                flags,
            } => self.render(doc, page, matrix, width, height, flags),
            Request::Crash if self.faults => std::process::abort(),
            Request::Crash => error(errors::BAD_REQUEST, "fault injection is off"),
        }
    }

    // `c_ulong` is `u32` on Windows and `u64` on Unix.
    #[allow(clippy::unnecessary_cast)]
    fn last_error(&self) -> u32 {
        // SAFETY: no preconditions; reads PDFium's thread-local error.
        (unsafe { (self.api.get_last_error)() }) as u32
    }

    fn open(&mut self, doc: u64, path: &str, password: Option<&str>, overlay: Vec<u8>) -> Body {
        let password = match password.map(CString::new).transpose() {
            Ok(p) => p,
            Err(_) => return error(errors::PASSWORD, "password contains NUL"),
        };
        let pw_ptr = password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        let opened = match source::open(Path::new(path), &self.governor, None) {
            Ok(o) => o,
            Err(e) => return error(errors::FILE, e.to_string()),
        };
        let file_len = opened.source.len();
        let mut bytes = Box::new(DocBytes {
            file: Some(opened.source),
            file_len,
            overlay,
        });
        let total = bytes.len();
        let (handle, access) = match c_ulong::try_from(total) {
            Ok(len) => {
                let mut access = Box::new(FileAccess {
                    len,
                    get_block,
                    param: &mut *bytes as *mut DocBytes as *mut c_void,
                });
                // SAFETY: `access` and the `DocBytes` it points to are boxed
                // and move into the `Doc`, which outlives the handle.
                let h = unsafe { (self.api.load_custom)(&mut *access, pw_ptr) };
                (h, access)
            }
            // Over 4 GiB on Windows (32-bit `unsigned long`): PDFium's own
            // 64-bit file stream, by path. No overlay is possible then.
            Err(_) => {
                if !bytes.overlay.is_empty() {
                    return error(
                        errors::TOO_LARGE,
                        "unsaved changes can't be shown for files of 4 GiB or more",
                    );
                }
                let Ok(cpath) = CString::new(path) else {
                    return error(errors::FILE, "path contains NUL");
                };
                bytes.file = None;
                // SAFETY: both strings are NUL-terminated and outlive the call.
                let h = unsafe { (self.api.load_document)(cpath.as_ptr(), pw_ptr) };
                let access = Box::new(FileAccess {
                    len: 0,
                    get_block,
                    param: std::ptr::null_mut(),
                });
                (h, access)
            }
        };
        if handle.is_null() {
            let code = self.last_error();
            return error(
                code,
                format!("PDFium could not open the file (error {code})"),
            );
        }
        let mut version: c_int = 0;
        // SAFETY: `handle` is a live document.
        let (pages, valid_xref) = unsafe {
            (self.api.file_version)(handle, &mut version);
            (
                (self.api.page_count)(handle).max(0) as u32,
                (self.api.valid_xref)(handle) != 0,
            )
        };
        self.docs.insert(
            doc,
            Doc {
                handle,
                _bytes: bytes,
                _access: access,
                pages: Vec::new(),
            },
        );
        Body::Opened {
            pages,
            valid_xref,
            version: version.max(0) as u32,
        }
    }

    fn close(&mut self, doc: u64) {
        if let Some(d) = self.docs.remove(&doc) {
            // SAFETY: the pages belong to this document, which is closed
            // last; nothing uses the handles afterwards.
            unsafe {
                for (_, p) in d.pages {
                    (self.api.close_page)(p);
                }
                (self.api.close_document)(d.handle);
            }
        }
    }

    fn page_sizes(&mut self, doc: u64, from: u32, to: u32) -> Body {
        let Some(d) = self.docs.get(&doc) else {
            return error(errors::NO_DOCUMENT, "no such document");
        };
        if to < from || (to - from) as usize > MAX_FRAME / 8 - 16 {
            return error(errors::BAD_REQUEST, "page range");
        }
        let mut sizes = Vec::with_capacity((to - from) as usize);
        for i in from..to {
            let mut s = FsSizeF::default();
            // SAFETY: a live document; PDFium checks the index.
            if unsafe { (self.api.size_by_index)(d.handle, i as c_int, &mut s) } == 0 {
                return error(errors::PAGE, format!("page {i}"));
            }
            sizes.push((s.width, s.height));
        }
        Body::PageSizes { sizes }
    }

    fn page(&mut self, doc: u64, index: u32) -> Result<Handle, Body> {
        let api = &self.api;
        let d = self
            .docs
            .get_mut(&doc)
            .ok_or_else(|| error(errors::NO_DOCUMENT, "no such document"))?;
        if let Some(i) = d.pages.iter().position(|&(n, _)| n == index) {
            let entry = d.pages.remove(i);
            d.pages.push(entry);
            return Ok(entry.1);
        }
        // SAFETY: a live document; PDFium checks the index.
        let page = unsafe { (api.load_page)(d.handle, index as c_int) };
        if page.is_null() {
            return Err(error(errors::PAGE, format!("page {index}")));
        }
        if d.pages.len() >= PAGE_CACHE {
            let (_, old) = d.pages.remove(0);
            // SAFETY: an evicted page of this document; no longer referenced.
            unsafe { (api.close_page)(old) };
        }
        d.pages.push((index, page));
        Ok(page)
    }

    fn render(
        &mut self,
        doc: u64,
        page: u32,
        m: Matrix,
        width: u32,
        height: u32,
        flags: u32,
    ) -> Body {
        let bytes = u64::from(width) * u64::from(height) * 4;
        if width == 0 || height == 0 || bytes + 64 > MAX_FRAME as u64 {
            return error(errors::TOO_LARGE, "bitmap size");
        }
        let page = match self.page(doc, page) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let mut rgba = vec![0u8; bytes as usize];
        let (w, h) = (width as c_int, height as c_int);
        let matrix = FsMatrix {
            a: m.a,
            b: m.b,
            c: m.c,
            d: m.d,
            e: m.e,
            f: m.f,
        };
        let clip = FsRectF {
            left: 0.0,
            top: 0.0,
            right: width as f32,
            bottom: height as f32,
        };
        // SAFETY: the bitmap wraps `rgba` (w·h·4 bytes, stride w·4) and is
        // destroyed before `rgba` moves; `page` is a live page.
        unsafe {
            let bmp = (self.api.bitmap_create_ex)(
                w,
                h,
                ffi::BITMAP_BGRA,
                rgba.as_mut_ptr().cast(),
                w * 4,
            );
            if bmp.is_null() {
                return error(errors::UNKNOWN, "bitmap");
            }
            (self.api.bitmap_fill)(bmp, 0, 0, w, h, 0xFFFF_FFFF);
            (self.api.render_matrix)(
                bmp,
                page,
                &matrix,
                &clip,
                flags as c_int | ffi::REVERSE_BYTE_ORDER,
            );
            (self.api.bitmap_destroy)(bmp);
        }
        Body::Pixels {
            width,
            height,
            rgba,
        }
    }
}

/// Private (pagefile-backed) bytes of this process: what it costs beyond
/// the mapped file.
pub fn private_bytes() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        // SAFETY: a pseudo-handle for this process and a correctly sized struct.
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0 {
            return c.PagefileUsage as u64;
        }
        0
    }
    #[cfg(target_os = "linux")]
    {
        // Anonymous resident memory; the mapped file is RssFile.
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("RssAnon:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            })
            .map_or(0, |kb| kb * 1024)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        0
    }
}

/// Runs the worker until stdin closes. Returns the process exit code.
pub fn run() -> i32 {
    let Some(library) = std::env::var_os(LIBRARY_ENV).map(PathBuf::from) else {
        eprintln!("quire pdf worker: {LIBRARY_ENV} is not set");
        return 2;
    };
    let api = match Api::load(&library) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("quire pdf worker: {e}");
            return 3;
        }
    };
    let mut worker = Worker {
        api,
        library,
        docs: HashMap::new(),
        // The chunk cache for network files; the host accounts for the
        // worker's private bytes as a whole.
        governor: MemoryGovernor::new(GovernorConfig::default()),
        faults: std::env::var_os(FAULTS_ENV).is_some_and(|v| v == "1"),
    };
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    loop {
        let frame = match protocol::read_frame(&mut input) {
            Ok(Some(f)) => f,
            Ok(None) => return 0,
            Err(e) => {
                eprintln!("quire pdf worker: {e}");
                return 4;
            }
        };
        let (id, body) = match protocol::decode_request(&frame) {
            Ok((id, req)) => (id, worker.handle(req)),
            Err(e) => (
                frame
                    .get(..8)
                    .and_then(|b| b.try_into().ok())
                    .map_or(0, u64::from_le_bytes),
                error(errors::BAD_REQUEST, e.to_string()),
            ),
        };
        let resp = Response {
            mem: private_bytes(),
            body,
        };
        if protocol::write_frame(&mut output, &protocol::encode_response(id, &resp)).is_err() {
            return 0;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let docs: Vec<u64> = self.docs.keys().copied().collect();
        for d in docs {
            self.close(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_text::source::MemorySource;

    #[test]
    fn reads_span_the_file_and_the_overlay() {
        let b = DocBytes {
            file: Some(Arc::new(MemorySource::new(b"abcdef".to_vec()))),
            file_len: 6,
            overlay: b"XYZ".to_vec(),
        };
        let mut buf = [0u8; 5];
        b.read(3, &mut buf).unwrap();
        assert_eq!(&buf, b"defXY");
        let mut buf = [0u8; 2];
        b.read(7, &mut buf).unwrap();
        assert_eq!(&buf, b"YZ");
        let mut buf = [0u8; 2];
        assert!(b.read(8, &mut buf).is_err());
        assert!(b.read(u64::MAX, &mut buf).is_err());
    }
}
