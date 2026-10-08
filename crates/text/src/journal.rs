//! Crash-recovery journal (§5.7).
//!
//! Every committed change is appended to a per-document journal as soon as
//! it happens, by a writer thread: each record reaches the operating system
//! at once (a killed process loses nothing that was handed over), and fsync
//! runs at most once per second (a power cut loses at most about a second).
//! After a clean save or close the journal is reset or deleted.
//!
//! Format: `QJRNL001`, a length-prefixed JSON header (the file, its stamp,
//! encoding), then records `[u32 length][u32 checksum][payload]`. Reading
//! stops at the first truncated or corrupt record: a crash mid-write loses
//! only that record. The parser treats the file as hostile input: lengths
//! are bounded and it never panics.
//!
//! A journal is locked by its writer. One whose lock can be taken belongs
//! to a process that is gone, and is offered for recovery.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::document::{LineEnding, Record};
use crate::search::Query;
use crate::source::FileStamp;

const MAGIC: &[u8; 8] = b"QJRNL001";
const MAX_HEADER: u32 = 1 << 20;
const MAX_RECORD: u32 = 1 << 30;
/// fsync at most this often.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// The file the edits apply to; `None` for an untitled document.
    pub path: Option<PathBuf>,
    pub stamp: Option<FileStamp>,
    /// Encoding the document was opened with (WHATWG name).
    pub encoding: String,
    pub created_ms: u64,
    pub pid: u32,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---- encoding ------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u64(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn le_code(le: Option<LineEnding>) -> u8 {
    match le {
        None => 0,
        Some(LineEnding::Lf) => 1,
        Some(LineEnding::Crlf) => 2,
        Some(LineEnding::Cr) => 3,
    }
}

/// Serializes a record with its timestamp.
pub fn encode_record(r: &Record, time_ms: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_u64(&mut out, time_ms);
    match r {
        Record::Edit {
            ops,
            kind,
            merged,
            selections,
        } => {
            out.push(1);
            out.push(*kind);
            out.push(u8::from(*merged));
            put_u32(&mut out, ops.len() as u32);
            for (at, del, ins) in ops {
                put_u64(&mut out, *at);
                put_u64(&mut out, *del);
                put_bytes(&mut out, ins);
            }
            put_u32(&mut out, selections.len() as u32);
            for (a, h) in selections {
                put_u64(&mut out, *a);
                put_u64(&mut out, *h);
            }
        }
        Record::Undo => out.push(2),
        Record::Redo => out.push(3),
        Record::Format {
            encoding,
            bom,
            line_ending,
        } => {
            out.push(4);
            put_bytes(&mut out, encoding.as_bytes());
            out.push(u8::from(*bom));
            out.push(le_code(*line_ending));
        }
        Record::ReplaceAll { query, replacement } => {
            out.push(5);
            let json = serde_json::to_vec(&(query, replacement)).expect("serializable");
            put_bytes(&mut out, &json);
        }
    }
    out
}

/// A bounds-checked reader over untrusted bytes.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|s| u32::from_le_bytes(s.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|s| u64::from_le_bytes(s.try_into().expect("8 bytes")))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.u64()?).ok()?;
        self.take(n)
    }
}

/// Parses one record payload; `None` if malformed.
pub fn decode_record(payload: &[u8]) -> Option<(Record, u64)> {
    let mut c = Cursor { b: payload, at: 0 };
    let time = c.u64()?;
    let r = match c.u8()? {
        1 => {
            let kind = c.u8()?;
            let merged = c.u8()? != 0;
            let n = c.u32()? as usize;
            // Each op needs at least 24 bytes: bound before allocating.
            if n > payload.len() / 24 {
                return None;
            }
            let mut ops = Vec::with_capacity(n);
            for _ in 0..n {
                let at = c.u64()?;
                let del = c.u64()?;
                ops.push((at, del, c.bytes()?.to_vec()));
            }
            let n = c.u32()? as usize;
            if n > payload.len() / 16 {
                return None;
            }
            let mut selections = Vec::with_capacity(n);
            for _ in 0..n {
                selections.push((c.u64()?, c.u64()?));
            }
            Record::Edit {
                ops,
                kind,
                merged,
                selections,
            }
        }
        2 => Record::Undo,
        3 => Record::Redo,
        4 => {
            let encoding = String::from_utf8(c.bytes()?.to_vec()).ok()?;
            let bom = c.u8()? != 0;
            let line_ending = match c.u8()? {
                0 => None,
                1 => Some(LineEnding::Lf),
                2 => Some(LineEnding::Crlf),
                3 => Some(LineEnding::Cr),
                _ => return None,
            };
            Record::Format {
                encoding,
                bom,
                line_ending,
            }
        }
        5 => {
            let (query, replacement): (Query, String) = serde_json::from_slice(c.bytes()?).ok()?;
            Record::ReplaceAll { query, replacement }
        }
        _ => return None,
    };
    (c.at == payload.len()).then_some((r, time))
}

fn checksum(payload: &[u8]) -> u32 {
    xxh3_64(payload) as u32
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    put_u32(&mut out, payload.len() as u32);
    put_u32(&mut out, checksum(payload));
    out.extend_from_slice(payload);
    out
}

fn header_bytes(h: &Header) -> Vec<u8> {
    let json = serde_json::to_vec(h).expect("serializable");
    let mut out = MAGIC.to_vec();
    put_u32(&mut out, json.len() as u32);
    out.extend_from_slice(&json);
    out
}

/// A parsed journal.
#[derive(Clone, Debug)]
pub struct Journal {
    pub header: Header,
    /// Records with their timestamps, in order.
    pub records: Vec<(Record, u64)>,
    /// Bytes after the last good record (a torn write), ignored.
    pub ignored_tail: u64,
}

/// Parses journal bytes. Never panics; stops at the first bad record.
pub fn parse(bytes: &[u8]) -> Option<Journal> {
    let mut c = Cursor { b: bytes, at: 0 };
    if c.take(8)? != MAGIC {
        return None;
    }
    let hl = c.u32()?;
    if hl > MAX_HEADER {
        return None;
    }
    let header: Header = serde_json::from_slice(c.take(hl as usize)?).ok()?;
    let mut records = Vec::new();
    loop {
        let start = c.at;
        let Some(len) = c.u32() else { break };
        let Some(sum) = c.u32() else { break };
        if len > MAX_RECORD {
            c.at = start;
            break;
        }
        let Some(payload) = c.take(len as usize) else {
            c.at = start;
            break;
        };
        if checksum(payload) != sum {
            c.at = start;
            break;
        }
        match decode_record(payload) {
            Some(r) => records.push(r),
            None => {
                c.at = start;
                break;
            }
        }
    }
    Some(Journal {
        header,
        records,
        ignored_tail: (bytes.len() - c.at) as u64,
    })
}

pub fn read(path: &Path) -> io::Result<Journal> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    parse(&bytes).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a journal"))
}

// ---- writer --------------------------------------------------------------

enum Msg {
    Records(Vec<(Record, u64)>),
    Reset(Header),
    Close { delete: bool },
}

/// Counters, for tests and the HUD.
#[derive(Debug, Default)]
pub struct JournalStats {
    pub records: AtomicU64,
    pub syncs: AtomicU64,
    pub bytes: AtomicU64,
}

/// Appends to one document's journal on a writer thread.
#[derive(Debug)]
pub struct JournalWriter {
    tx: mpsc::Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    pub path: PathBuf,
    pub stats: Arc<JournalStats>,
}

impl JournalWriter {
    /// Creates `path` (locked for as long as the writer lives) with `header`.
    pub fn create(path: &Path, header: &Header) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.try_lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        file.write_all(&header_bytes(header))?;
        file.sync_data()?;
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(JournalStats::default());
        let s2 = Arc::clone(&stats);
        let p2 = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("quire-journal".into())
            .spawn(move || writer_loop(file, rx, &s2, &p2))?;
        Ok(Self {
            tx,
            thread: Some(thread),
            path: path.to_path_buf(),
            stats,
        })
    }

    /// Queues records for writing (returns at once).
    pub fn append(&self, records: Vec<Record>) {
        if records.is_empty() {
            return;
        }
        let t = now_ms();
        let _ = self
            .tx
            .send(Msg::Records(records.into_iter().map(|r| (r, t)).collect()));
    }

    /// Starts over with a new header (after a save: the file on disk now
    /// holds every change).
    pub fn reset(&self, header: Header) {
        let _ = self.tx.send(Msg::Reset(header));
    }

    /// Stops the writer; deletes the journal on a clean close.
    pub fn close(mut self, delete: bool) {
        let _ = self.tx.send(Msg::Close { delete });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        // Not closed explicitly: flush, keep the file (it may be needed).
        let _ = self.tx.send(Msg::Close { delete: false });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn writer_loop(mut file: File, rx: mpsc::Receiver<Msg>, stats: &JournalStats, path: &Path) {
    let mut last_sync = Instant::now();
    let mut dirty = false;
    let sync = |file: &File, dirty: &mut bool, last: &mut Instant| {
        if *dirty {
            let _ = file.sync_data();
            stats.syncs.fetch_add(1, Ordering::Relaxed);
            *dirty = false;
            *last = Instant::now();
        }
    };
    loop {
        let wait = if dirty {
            SYNC_INTERVAL.saturating_sub(last_sync.elapsed())
        } else {
            Duration::from_secs(3600)
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Records(rs)) => {
                let mut buf = Vec::new();
                for (r, t) in &rs {
                    buf.extend_from_slice(&frame(&encode_record(r, *t)));
                }
                // Straight to the OS: a killed process loses nothing queued here.
                if let Err(err) = file.write_all(&buf) {
                    tracing::error!(%err, path = %path.display(), "journal write failed");
                }
                stats.records.fetch_add(rs.len() as u64, Ordering::Relaxed);
                stats.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
                dirty = true;
                if last_sync.elapsed() >= SYNC_INTERVAL {
                    sync(&file, &mut dirty, &mut last_sync);
                }
            }
            Ok(Msg::Reset(h)) => {
                let r = file
                    .set_len(0)
                    .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
                    .and_then(|()| file.write_all(&header_bytes(&h)));
                if let Err(err) = r {
                    tracing::error!(%err, "journal reset failed");
                }
                dirty = true;
                sync(&file, &mut dirty, &mut last_sync);
            }
            Ok(Msg::Close { delete }) => {
                sync(&file, &mut dirty, &mut last_sync);
                let _ = file.unlock();
                drop(file);
                if delete {
                    let _ = std::fs::remove_file(path);
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => sync(&file, &mut dirty, &mut last_sync),
            Err(RecvTimeoutError::Disconnected) => {
                sync(&file, &mut dirty, &mut last_sync);
                return;
            }
        }
    }
}

// ---- recovery ------------------------------------------------------------

/// A journal left by a process that is gone.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Orphan {
    pub journal: PathBuf,
    pub header: Header,
    pub records: usize,
    pub last_change_ms: Option<u64>,
}

/// Journals in `dir` that no live writer holds and that have changes.
pub fn orphans(dir: &Path) -> Vec<Orphan> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "qj") {
            continue;
        }
        let Ok(f) = File::open(&p) else { continue };
        if f.try_lock().is_err() {
            continue; // a live writer holds it
        }
        let _ = f.unlock();
        drop(f);
        match read(&p) {
            Ok(j) if !j.records.is_empty() => out.push(Orphan {
                journal: p,
                header: j.header,
                records: j.records.len(),
                last_change_ms: j.records.last().map(|r| r.1),
            }),
            // Empty or unreadable: nothing to recover.
            _ => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    out
}

#[derive(Debug)]
pub enum RecoverError {
    /// The file on disk is no longer the one the edits were made to.
    FileChanged,
    Io(io::Error),
}

impl std::fmt::Display for RecoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoverError::FileChanged => {
                f.write_str("the file changed on disk after these edits were made")
            }
            RecoverError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RecoverError {}

impl From<io::Error> for RecoverError {
    fn from(e: io::Error) -> Self {
        RecoverError::Io(e)
    }
}

/// Re-opens the journaled file (refusing if it changed since) and replays
/// the journal. The result is dirty until saved.
pub fn recover(
    j: &Journal,
    governor: &quire_core::memory::MemoryGovernor,
    temp_dir: &Path,
    threads: usize,
) -> Result<crate::file::TextFile, RecoverError> {
    let mut file = match &j.header.path {
        None => crate::file::TextFile::untitled(governor, temp_dir.to_path_buf()),
        Some(path) => {
            let now = FileStamp::of_path(path)?;
            if j.header.stamp != Some(now) {
                return Err(RecoverError::FileChanged);
            }
            let encoding = encoding_rs::Encoding::for_label(j.header.encoding.as_bytes());
            crate::file::open(
                path,
                governor,
                None,
                &crate::file::OpenSettings {
                    encoding,
                    temp_dir: temp_dir.to_path_buf(),
                    threads,
                    progress: &|_| true,
                },
            )?
        }
    };
    file.doc.read_only = false;
    for (r, _) in &j.records {
        file.doc.apply_record(r)?;
    }
    Ok(file)
}

/// A file name for a new journal of `path`.
pub fn journal_name(path: Option<&Path>) -> String {
    let key = path.map_or_else(String::new, |p| p.to_string_lossy().into_owned());
    format!(
        "{:016x}-{}-{}.qj",
        xxh3_64(key.as_bytes()),
        std::process::id(),
        now_ms()
    )
}

#[cfg(test)]
mod tests;
