//! The host side of the worker pool (§5.2, ADR-0036).
//!
//! Workers start on first use. Every request goes to the alive worker with
//! the fewest requests in flight; a worker opens a document the first time
//! it gets a request for it (and again after the document's overlay
//! changes). When a worker dies (crash, decode error, timeout), its pending
//! requests fail, its slot is refilled on the next request, and a request
//! that died with it is retried once on a fresh worker: the user sees no
//! more than a tile rendering again.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::protocol::{self, Body, Matrix, PROTOCOL_VERSION, Request, errors};
use crate::worker::{FAULTS_ENV, LIBRARY_ENV};

/// How to start a worker process.
#[derive(Clone, Debug)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The PDFium library the worker loads.
    pub library: PathBuf,
    /// Lets tests make a worker crash on request.
    pub faults: bool,
}

#[derive(Clone, Debug)]
pub struct PoolConfig {
    pub workers: usize,
    pub command: WorkerCommand,
    /// A request that takes longer kills its worker (a page that hangs
    /// PDFium must not hold a worker forever).
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PdfError {
    /// The document needs a password, or the one given is wrong.
    Password,
    /// The file couldn't be read.
    File(String),
    /// Not a PDF PDFium can open, or a broken page.
    Format {
        code: u32,
        message: String,
    },
    /// The worker died while handling this, twice.
    WorkerCrashed,
    Timeout,
    /// The worker sent something the host can't decode, or wouldn't start.
    Worker(String),
    UnknownDocument,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Password => write!(f, "password required"),
            PdfError::File(m) => write!(f, "file: {m}"),
            PdfError::Format { code, message } => write!(f, "{message} ({code})"),
            PdfError::WorkerCrashed => write!(f, "the PDF worker crashed"),
            PdfError::Timeout => write!(f, "the PDF worker timed out"),
            PdfError::Worker(m) => write!(f, "PDF worker: {m}"),
            PdfError::UnknownDocument => write!(f, "unknown document"),
        }
    }
}

impl std::error::Error for PdfError {}

impl PdfError {
    fn from_code(code: u32, message: String) -> Self {
        match code {
            errors::PASSWORD => PdfError::Password,
            errors::FILE => PdfError::File(message),
            _ => PdfError::Format { code, message },
        }
    }
}

/// What a document looks like once opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocInfo {
    pub pages: u32,
    /// False when PDFium rebuilt a broken cross-reference table: the UI
    /// shows the "repaired" banner (§6.6).
    pub valid_xref: bool,
    pub version: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub slots: usize,
    pub alive: usize,
    pub spawned: u32,
    pub crashes: u32,
    pub in_flight: usize,
    /// Sum of the alive workers' private bytes, as last reported.
    pub private_bytes: u64,
}

type Reply = Result<Body, PdfError>;

struct Worker {
    stdin: Mutex<BufWriter<ChildStdin>>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    alive: AtomicBool,
    in_flight: AtomicUsize,
    private_bytes: AtomicU64,
    /// Documents open in this worker, with the revision they were opened at.
    open: Mutex<HashMap<u64, u64>>,
}

impl Worker {
    fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.child.lock().kill();
    }

    /// Fails every pending request; called once the worker is gone.
    fn fail_pending(&self, err: &PdfError) {
        self.alive.store(false, Ordering::SeqCst);
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(err.clone()));
        }
    }
}

#[derive(Clone)]
struct DocSpec {
    path: String,
    password: Option<String>,
    overlay: Arc<Vec<u8>>,
    revision: u64,
}

struct Inner {
    config: PoolConfig,
    slots: Vec<Mutex<Option<Arc<Worker>>>>,
    docs: Mutex<HashMap<u64, DocSpec>>,
    next_id: AtomicU64,
    next_doc: AtomicU64,
    spawned: AtomicU32,
    crashes: AtomicU32,
    job: job::Job,
}

/// A pool of PDFium worker processes. Cheap to clone.
#[derive(Clone)]
pub struct Pool(Arc<Inner>);

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Pool {
    pub fn new(config: PoolConfig) -> Self {
        let n = config.workers.max(1);
        Pool(Arc::new(Inner {
            slots: (0..n).map(|_| Mutex::new(None)).collect(),
            config,
            docs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_doc: AtomicU64::new(1),
            spawned: AtomicU32::new(0),
            crashes: AtomicU32::new(0),
            job: job::Job::new(),
        }))
    }

    pub fn stats(&self) -> PoolStats {
        let mut s = PoolStats {
            slots: self.0.slots.len(),
            spawned: self.0.spawned.load(Ordering::Relaxed),
            crashes: self.0.crashes.load(Ordering::Relaxed),
            ..PoolStats::default()
        };
        for slot in &self.0.slots {
            if let Some(w) = slot.lock().as_ref()
                && w.alive.load(Ordering::Relaxed)
            {
                s.alive += 1;
                s.in_flight += w.in_flight.load(Ordering::Relaxed);
                s.private_bytes += w.private_bytes.load(Ordering::Relaxed);
            }
        }
        s
    }

    /// Opens a document on one worker (the others open it on demand) and
    /// returns its id and what PDFium reports about it.
    pub fn open_document(
        &self,
        path: &str,
        password: Option<&str>,
    ) -> Result<(u64, DocInfo), PdfError> {
        let doc = self.0.next_doc.fetch_add(1, Ordering::Relaxed);
        self.0.docs.lock().insert(
            doc,
            DocSpec {
                path: path.to_owned(),
                password: password.map(str::to_owned),
                overlay: Arc::new(Vec::new()),
                revision: 0,
            },
        );
        match self.with_retry(doc, |w| self.ensure_open(&w, doc)) {
            Ok(info) => Ok((doc, info)),
            Err(e) => {
                self.0.docs.lock().remove(&doc);
                Err(e)
            }
        }
    }

    /// Replaces the bytes appended to the document as workers see it; each
    /// worker reopens it before its next request for it.
    pub fn set_overlay(&self, doc: u64, overlay: Vec<u8>) -> Result<(), PdfError> {
        let mut docs = self.0.docs.lock();
        let spec = docs.get_mut(&doc).ok_or(PdfError::UnknownDocument)?;
        spec.overlay = Arc::new(overlay);
        spec.revision += 1;
        Ok(())
    }

    pub fn close_document(&self, doc: u64) {
        self.0.docs.lock().remove(&doc);
        for slot in &self.0.slots {
            let w = slot.lock().clone();
            if let Some(w) = w
                && w.open.lock().remove(&doc).is_some()
            {
                let _ = self.send(&w, &Request::Close { doc });
            }
        }
    }

    pub fn page_sizes(&self, doc: u64, from: u32, to: u32) -> Result<Vec<(f32, f32)>, PdfError> {
        match self.request(doc, Request::PageSizes { doc, from, to })? {
            Body::PageSizes { sizes } => Ok(sizes),
            other => Err(unexpected(&other)),
        }
    }

    /// Renders an opaque RGBA bitmap of `width × height` pixels.
    pub fn render(
        &self,
        doc: u64,
        page: u32,
        matrix: Matrix,
        width: u32,
        height: u32,
        flags: u32,
    ) -> Result<Vec<u8>, PdfError> {
        let req = Request::Render {
            doc,
            page,
            matrix,
            width,
            height,
            flags,
        };
        match self.request(doc, req)? {
            Body::Pixels { rgba, .. } => Ok(rgba),
            other => Err(unexpected(&other)),
        }
    }

    /// Makes one alive worker abort (tests of crash recovery; the worker
    /// must have been started with `faults`).
    pub fn crash_a_worker(&self) -> Result<(), PdfError> {
        let w = self.worker()?;
        let _ = self.send(&w, &Request::Crash);
        Ok(())
    }

    fn request(&self, doc: u64, req: Request) -> Result<Body, PdfError> {
        self.with_retry(doc, |w| {
            self.ensure_open(&w, doc)?;
            self.call(&w, &req)
        })
    }

    /// Runs `f` on a worker; if the worker dies under it, once more on
    /// another (or a restarted) one.
    fn with_retry<T>(
        &self,
        doc: u64,
        f: impl Fn(Arc<Worker>) -> Result<T, PdfError>,
    ) -> Result<T, PdfError> {
        let mut last = PdfError::WorkerCrashed;
        for _ in 0..2 {
            if !self.0.docs.lock().contains_key(&doc) {
                return Err(PdfError::UnknownDocument);
            }
            let w = self.worker()?;
            match f(w) {
                Err(PdfError::WorkerCrashed) => last = PdfError::WorkerCrashed,
                r => return r,
            }
        }
        Err(last)
    }

    fn ensure_open(&self, w: &Arc<Worker>, doc: u64) -> Result<DocInfo, PdfError> {
        let spec = self
            .0
            .docs
            .lock()
            .get(&doc)
            .cloned()
            .ok_or(PdfError::UnknownDocument)?;
        // Held across the round trip so two callers don't open it twice.
        let mut open = w.open.lock();
        let req = Request::Open {
            doc,
            path: spec.path.clone(),
            password: spec.password.clone(),
            overlay: (*spec.overlay).clone(),
        };
        if open.get(&doc) == Some(&spec.revision) {
            // Already open; the caller doesn't need the info again.
            return Ok(DocInfo {
                pages: 0,
                valid_xref: true,
                version: 0,
            });
        }
        match self.call(w, &req)? {
            Body::Opened {
                pages,
                valid_xref,
                version,
            } => {
                open.insert(doc, spec.revision);
                Ok(DocInfo {
                    pages,
                    valid_xref,
                    version,
                })
            }
            other => Err(unexpected(&other)),
        }
    }

    /// The alive worker with the fewest requests in flight, starting
    /// workers for empty or dead slots first.
    fn worker(&self) -> Result<Arc<Worker>, PdfError> {
        let mut best: Option<Arc<Worker>> = None;
        for slot in &self.0.slots {
            let mut s = slot.lock();
            let alive = s.as_ref().is_some_and(|w| w.alive.load(Ordering::SeqCst));
            if !alive {
                if s.is_some() {
                    self.0.crashes.fetch_add(1, Ordering::Relaxed);
                }
                *s = None;
                // Start a worker only where it helps: when none is idle.
                if best
                    .as_ref()
                    .is_some_and(|b| b.in_flight.load(Ordering::Relaxed) == 0)
                {
                    continue;
                }
                *s = Some(self.spawn()?);
            }
            let w = s.as_ref().map(Arc::clone);
            if let Some(w) = w {
                let load = w.in_flight.load(Ordering::Relaxed);
                if best
                    .as_ref()
                    .is_none_or(|b| load < b.in_flight.load(Ordering::Relaxed))
                {
                    best = Some(w);
                }
            }
        }
        best.ok_or_else(|| PdfError::Worker("no worker".into()))
    }

    fn spawn(&self) -> Result<Arc<Worker>, PdfError> {
        let c = &self.0.config.command;
        let mut cmd = Command::new(&c.program);
        cmd.args(&c.args)
            .env(LIBRARY_ENV, &c.library)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if c.faults {
            cmd.env(FAULTS_ENV, "1");
        } else {
            cmd.env_remove(FAULTS_ENV);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: a console worker under a GUI app gets no window.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| PdfError::Worker(format!("starting {}: {e}", c.program.display())))?;
        self.0.job.assign(&child);
        self.0.spawned.fetch_add(1, Ordering::Relaxed);
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(PdfError::Worker("no pipes".into()));
        };
        let w = Arc::new(Worker {
            stdin: Mutex::new(BufWriter::new(stdin)),
            child: Mutex::new(child),
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            in_flight: AtomicUsize::new(0),
            private_bytes: AtomicU64::new(0),
            open: Mutex::new(HashMap::new()),
        });
        let reader = Arc::clone(&w);
        std::thread::Builder::new()
            .name("pdf-worker-reader".into())
            .spawn(move || read_loop(&reader, stdout))
            .map_err(|e| PdfError::Worker(e.to_string()))?;
        match self.call(
            &w,
            &Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
        ) {
            Ok(Body::Hello { protocol, .. }) if protocol == PROTOCOL_VERSION => Ok(w),
            Ok(other) => {
                w.kill();
                Err(unexpected(&other))
            }
            Err(e) => {
                w.kill();
                Err(PdfError::Worker(format!("handshake: {e}")))
            }
        }
    }

    fn send(&self, w: &Worker, req: &Request) -> Result<mpsc::Receiver<Reply>, PdfError> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        w.pending.lock().insert(id, tx);
        if !w.alive.load(Ordering::SeqCst) {
            w.pending.lock().remove(&id);
            return Err(PdfError::WorkerCrashed);
        }
        let frame = protocol::encode_request(id, req);
        if protocol::write_frame(&mut *w.stdin.lock(), &frame).is_err() {
            w.pending.lock().remove(&id);
            w.kill();
            return Err(PdfError::WorkerCrashed);
        }
        Ok(rx)
    }

    fn call(&self, w: &Worker, req: &Request) -> Reply {
        w.in_flight.fetch_add(1, Ordering::Relaxed);
        let r = self
            .send(w, req)
            .and_then(|rx| match rx.recv_timeout(self.0.config.timeout) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    tracing::warn!(?req, "PDF worker timed out; killing it");
                    w.kill();
                    Err(PdfError::Timeout)
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(PdfError::WorkerCrashed),
            });
        w.in_flight.fetch_sub(1, Ordering::Relaxed);
        match r? {
            Body::Error { code, message } => Err(PdfError::from_code(code, message)),
            body => Ok(body),
        }
    }
}

fn unexpected(body: &Body) -> PdfError {
    PdfError::Worker(format!(
        "unexpected response {:?}",
        std::mem::discriminant(body)
    ))
}

fn read_loop(w: &Worker, stdout: std::process::ChildStdout) {
    let mut r = BufReader::new(stdout);
    let err = loop {
        let frame = match protocol::read_frame(&mut r) {
            Ok(Some(f)) => f,
            Ok(None) | Err(_) => break PdfError::WorkerCrashed,
        };
        match protocol::decode_response(&frame) {
            Ok((id, resp)) => {
                w.private_bytes.store(resp.mem, Ordering::Relaxed);
                if let Some(tx) = w.pending.lock().remove(&id) {
                    let _ = tx.send(Ok(resp.body));
                }
            }
            Err(e) => {
                // A worker that sends garbage may be compromised: stop it.
                tracing::error!(%e, "PDF worker sent a bad frame; killing it");
                w.kill();
                break PdfError::WorkerCrashed;
            }
        }
    };
    let status = w.child.lock().wait();
    tracing::info!(?status, "PDF worker exited");
    w.fail_pending(&err);
}

impl Drop for Inner {
    fn drop(&mut self) {
        for slot in &self.slots {
            if let Some(w) = slot.lock().take() {
                w.kill();
            }
        }
    }
}

/// Windows: every worker is in one Job Object that kills them when the
/// host's handle closes, so a crashed host never leaves workers behind.
mod job {
    #[cfg(windows)]
    pub struct Job(windows_sys::Win32::Foundation::HANDLE);

    // SAFETY: a job handle is a kernel handle, usable from any thread.
    #[cfg(windows)]
    unsafe impl Send for Job {}
    // SAFETY: as above; the handle is only read after creation.
    #[cfg(windows)]
    unsafe impl Sync for Job {}

    #[cfg(windows)]
    impl Job {
        pub fn new() -> Self {
            use windows_sys::Win32::System::JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            };
            // SAFETY: plain Win32 calls with valid arguments; a zeroed
            // limit struct with one flag set is a valid configuration.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if !h.is_null() {
                    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    SetInformationJobObject(
                        h,
                        JobObjectExtendedLimitInformation,
                        (&raw const info).cast(),
                        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                }
                Job(h)
            }
        }

        pub fn assign(&self, child: &std::process::Child) {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            if !self.0.is_null() {
                // SAFETY: both handles are valid for the call.
                unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
            }
        }
    }

    #[cfg(windows)]
    impl Drop for Job {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: our handle, closed once.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
            }
        }
    }

    #[cfg(not(windows))]
    pub struct Job;

    #[cfg(not(windows))]
    impl Job {
        pub fn new() -> Self {
            Job
        }
        pub fn assign(&self, _child: &std::process::Child) {}
    }
}
