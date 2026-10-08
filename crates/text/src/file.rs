//! Opening and saving text files (§5.5, §5.8, §6.1).
//!
//! **Save** writes a temp file in the target's directory, fsyncs it and
//! atomically replaces the target (`ReplaceFileW` on Windows, which keeps
//! the original's attributes and ACLs; `rename(2)` elsewhere, with the
//! permissions copied and the directory fsynced). Following §5.8 exactly:
//!
//! 1. The new content streams from the old mapping into the temp file.
//!    Untouched ranges of the original are copied raw (byte-for-byte, even
//!    for legacy encodings); everything else is encoded. The new file's
//!    line index is built while it is written.
//! 2. Undo-history ranges that reference the old mapping are copied into
//!    memory (bounded; the oldest steps are dropped past the bound).
//! 3. The original is unmapped.
//! 4. The temp file replaces it.
//! 5. The new file is mapped and the document re-pointed at it.
//!
//! Each save returns a [`SaveReport`]. A same-format save must report no
//! losses; anything that can't be written faithfully (characters the
//! target encoding lacks) fails the save unless the caller confirmed.

#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use encoding_rs::{EncoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE};
use quire_core::memory::{DocId, MemoryGovernor};
use serde::Serialize;

use crate::buffer::{BufferEntry, ORIGINAL, Snapshot, TextBuffer};
use crate::document::{LineEnding, SaveFormat, TextDocument};
use crate::encoding::SAMPLE;
use crate::encoding::{DetectedBy, Detection, detect};
use crate::history::Change;
use crate::index::{ChunkIndex, IndexBuilder};
use crate::source::{self, FileStamp, MemorySource, RemoteKind, Source, SourceKind};
use crate::transcode::{Transcoded, transcode};
use crate::tree::{BufferId, Piece, PieceTree};

/// Undo data copied out of an old mapping at save, at most.
pub const SPILL_MAX: u64 = 256 << 20;

/// How the document's original relates to the file on disk.
#[derive(Clone, Debug)]
pub struct Origin {
    /// The file's raw bytes (BOM included).
    pub raw: Arc<dyn Source>,
    pub encoding: &'static Encoding,
    pub bom_len: u64,
    /// For non-UTF-8 files: the decoded text and its segment table.
    pub transcoded: Option<Arc<Transcoded>>,
}

impl Origin {
    /// Whether untouched original ranges can be copied raw in `format`.
    fn raw_copy(&self, format: &SaveFormat) -> bool {
        format.encoding == self.encoding
            && format.line_ending.is_none()
            && self
                .transcoded
                .as_ref()
                .is_none_or(|t| t.supports_raw_copy())
    }
}

/// An open text file.
#[derive(Debug)]
pub struct TextFile {
    pub doc: TextDocument,
    pub path: Option<PathBuf>,
    pub origin: Option<Origin>,
    pub stamp: Option<FileStamp>,
    pub detection: Detection,
    pub source_kind: SourceKind,
    pub remote: Option<RemoteKind>,
    pub governor: MemoryGovernor,
    pub doc_id: Option<DocId>,
}

pub struct OpenSettings<'a> {
    /// Decode with this encoding instead of detecting one.
    pub encoding: Option<&'static Encoding>,
    /// Where transcoded and derived buffers are written.
    pub temp_dir: PathBuf,
    pub threads: usize,
    /// Called with raw bytes decoded so far; false cancels.
    pub progress: &'a dyn Fn(u64) -> bool,
}

impl std::fmt::Debug for OpenSettings<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenSettings")
            .field("encoding", &self.encoding.map(Encoding::name))
            .field("temp_dir", &self.temp_dir)
            .finish()
    }
}

/// The dominant line ending of a sample (the platform's for none).
pub fn dominant_eol(sample: &[u8]) -> LineEnding {
    let (_, s) = crate::index::scan(sample, None, None);
    if s.crlf == 0 && s.lf == 0 && s.cr == 0 {
        return if cfg!(windows) {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        };
    }
    if s.crlf >= s.lf && s.crlf >= s.cr {
        LineEnding::Crlf
    } else if s.lf >= s.cr {
        LineEnding::Lf
    } else {
        LineEnding::Cr
    }
}

/// Opens `path`. For UTF-8 files the document maps the file directly and
/// its line index starts empty (fill it with `index::index_in_background`
/// on [`TextFile::original_index`]). Other encodings are transcoded first.
pub fn open(
    path: &Path,
    governor: &MemoryGovernor,
    doc_id: Option<DocId>,
    opts: &OpenSettings<'_>,
) -> io::Result<TextFile> {
    let opened = source::open(path, governor, doc_id)?;
    let raw = opened.source;
    let head = raw.read_vec(0, raw.len().min(SAMPLE as u64) as usize)?;
    let mut detection = detect(&head, raw.len() as usize == head.len());
    if let Some(e) = opts.encoding {
        let bom = Encoding::for_bom(&head)
            .filter(|(b, _)| *b == e)
            .map_or(0, |(_, n)| n);
        detection = Detection {
            encoding: e,
            bom_len: bom,
            by: DetectedBy::User,
            binary: detection.binary,
        };
    }
    let (origin, buf) = from_raw(raw, detection, &opts.temp_dir, opts.threads, opts.progress)?;
    let eol = dominant_eol(&buf.snapshot().read(0..buf.len().min(SAMPLE as u64))?);
    let format = SaveFormat {
        encoding: detection.encoding,
        bom: detection.bom_len > 0,
        line_ending: None,
    };
    let mut doc = TextDocument::new(buf, format, eol);
    doc.read_only = detection.binary;
    doc.temp_dir = opts.temp_dir.clone();
    Ok(TextFile {
        doc,
        path: Some(path.to_path_buf()),
        origin: Some(origin),
        stamp: Some(opened.stamp),
        detection,
        source_kind: opened.kind,
        remote: opened.remote,
        governor: governor.clone(),
        doc_id,
    })
}

fn from_raw(
    raw: Arc<dyn Source>,
    detection: Detection,
    temp_dir: &Path,
    threads: usize,
    progress: &dyn Fn(u64) -> bool,
) -> io::Result<(Origin, TextBuffer)> {
    let bom = detection.bom_len as u64;
    if detection.encoding == UTF_8 {
        let index = Arc::new(ChunkIndex::unindexed(raw.len()));
        let buf = TextBuffer::with_original(Arc::clone(&raw), index, bom);
        return Ok((
            Origin {
                raw,
                encoding: UTF_8,
                bom_len: bom,
                transcoded: None,
            },
            buf,
        ));
    }
    let t = Arc::new(transcode(
        Arc::clone(&raw),
        detection.encoding,
        bom,
        temp_dir,
        threads,
        progress,
    )?);
    let index = Arc::new(ChunkIndex::unindexed(t.utf8.len()));
    let buf = TextBuffer::new(Arc::clone(&t.utf8), index);
    Ok((
        Origin {
            raw,
            encoding: detection.encoding,
            bom_len: bom,
            transcoded: Some(t),
        },
        buf,
    ))
}

impl TextFile {
    /// A new, empty, unsaved document.
    pub fn untitled(governor: &MemoryGovernor, temp_dir: PathBuf) -> Self {
        let eol = dominant_eol(b"");
        let mut doc = TextDocument::new(TextBuffer::from_bytes(b""), SaveFormat::default(), eol);
        doc.temp_dir = temp_dir;
        Self {
            doc,
            path: None,
            origin: None,
            stamp: None,
            detection: Detection {
                encoding: UTF_8,
                bom_len: 0,
                by: DetectedBy::Default,
                binary: false,
            },
            source_kind: SourceKind::Memory,
            remote: None,
            governor: governor.clone(),
            doc_id: None,
        }
    }

    /// Buffer 0 and its index, for background indexing.
    pub fn original_index(&self) -> BufferEntry {
        self.doc.buffer().buffers().get(ORIGINAL)
    }
}

// ---- writing ------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OnUnmappable {
    /// Fail the save and report what can't be written.
    Fail,
    /// Write '?' for each such character (the user confirmed).
    Replace,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReport {
    pub path: String,
    pub bytes_written: u64,
    /// Bytes copied unchanged from the original.
    pub raw_copied: u64,
    pub encoding: String,
    pub unmappable: u64,
    /// Invalid UTF-8 bytes in the document that couldn't be carried into a
    /// different encoding.
    pub invalid_bytes: u64,
    /// What couldn't be preserved; empty for a faithful save.
    pub losses: Vec<String>,
    /// Undo steps dropped to bound memory or because the text changed.
    pub history_trimmed: u64,
    pub millis: u64,
}

#[derive(Debug)]
pub enum SaveError {
    /// Characters the target encoding can't represent; nothing was written.
    Unmappable {
        count: u64,
        sample: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Unmappable { count, sample } => {
                write!(
                    f,
                    "{count} characters can't be written in this encoding (first: {sample:?})"
                )
            }
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// Converts line breaks in a UTF-8 stream (CR, LF and CRLF all become one
/// break), keeping a CR at a block end until the next byte is known.
struct EolConv {
    target: Option<LineEnding>,
    pending_cr: bool,
}

impl EolConv {
    fn convert(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let Some(t) = self.target else {
            out.extend_from_slice(input);
            return;
        };
        for &b in input {
            if self.pending_cr {
                self.pending_cr = false;
                out.extend_from_slice(t.bytes());
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => self.pending_cr = true,
                b'\n' => out.extend_from_slice(t.bytes()),
                _ => out.push(b),
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<u8>) {
        if self.pending_cr
            && let Some(t) = self.target
        {
            out.extend_from_slice(t.bytes());
        }
        self.pending_cr = false;
    }
}

/// Encodes UTF-8 to the target encoding, carrying an incomplete trailing
/// sequence over to the next block.
struct Enc {
    encoding: &'static Encoding,
    encoder: Option<encoding_rs::Encoder>,
    carry: Vec<u8>,
    on_unmappable: OnUnmappable,
    unmappable: u64,
    first_unmappable: Option<char>,
    invalid: u64,
}

impl Enc {
    fn new(encoding: &'static Encoding, on_unmappable: OnUnmappable) -> Self {
        let legacy = encoding != UTF_8 && encoding != UTF_16LE && encoding != UTF_16BE;
        Self {
            encoding,
            encoder: legacy.then(|| encoding.new_encoder()),
            carry: Vec::new(),
            on_unmappable,
            unmappable: 0,
            first_unmappable: None,
            invalid: 0,
        }
    }

    /// Encodes `input` (plus carry) into `out`.
    fn encode(&mut self, input: &[u8], last: bool, out: &mut Vec<u8>) -> Result<(), SaveError> {
        if self.encoding == UTF_8 {
            out.extend_from_slice(input);
            return Ok(());
        }
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(input);
        let keep = if last {
            data.len()
        } else {
            data.len() - incomplete_tail(&data)
        };
        self.carry = data[keep..].to_vec();
        let text = String::from_utf8_lossy(&data[..keep]);
        if let std::borrow::Cow::Owned(_) = text {
            let invalid = crate::chars::chars(&data[..keep])
                .filter(|c| c.2.is_none())
                .count();
            self.invalid += invalid as u64;
        }
        if self.encoding == UTF_16LE || self.encoding == UTF_16BE {
            for u in text.encode_utf16() {
                out.extend_from_slice(&if self.encoding == UTF_16LE {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                });
            }
            return Ok(());
        }
        let encoder = self.encoder.as_mut().expect("legacy encoder");
        let mut rest: &str = &text;
        let mut buf = vec![0u8; rest.len() * 4 + 64];
        loop {
            let (result, read, written) =
                encoder.encode_from_utf8_without_replacement(rest, &mut buf, last);
            out.extend_from_slice(&buf[..written]);
            rest = &rest[read..];
            match result {
                EncoderResult::InputEmpty => return Ok(()),
                EncoderResult::OutputFull => buf.resize(buf.len() * 2, 0),
                EncoderResult::Unmappable(c) => {
                    self.unmappable += 1;
                    self.first_unmappable.get_or_insert(c);
                    if self.on_unmappable == OnUnmappable::Fail {
                        continue;
                    }
                    let (_, _, w) =
                        encoder.encode_from_utf8_without_replacement("?", &mut buf, false);
                    out.extend_from_slice(&buf[..w]);
                }
            }
        }
    }
}

/// Length of an incomplete UTF-8 sequence at the end of `data` (0-3).
fn incomplete_tail(data: &[u8]) -> usize {
    for back in 1..=3.min(data.len()) {
        let b = data[data.len() - back];
        if b & 0xC0 == 0x80 {
            continue;
        }
        let need = match b {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if need > back { back } else { 0 };
    }
    0
}

fn bom_bytes(e: &'static Encoding) -> &'static [u8] {
    if e == UTF_8 {
        b"\xEF\xBB\xBF"
    } else if e == UTF_16LE {
        b"\xFF\xFE"
    } else if e == UTF_16BE {
        b"\xFE\xFF"
    } else {
        b""
    }
}

struct Sink<W: Write> {
    w: W,
    index: IndexBuilder,
    written: u64,
}

impl<W: Write> Sink<W> {
    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.w.write_all(b)?;
        self.index.push(b);
        self.written += b.len() as u64;
        Ok(())
    }
}

struct WriteStats {
    written: u64,
    raw_copied: u64,
    unmappable: u64,
    first_unmappable: Option<char>,
    invalid: u64,
    index: ChunkIndex,
}

/// Streams `snap` in `format` into `w`.
fn write_content<W: Write>(
    w: W,
    snap: &Snapshot,
    origin: Option<&Origin>,
    format: &SaveFormat,
    on_unmappable: OnUnmappable,
) -> Result<(WriteStats, W), SaveError> {
    const BLOCK: u64 = 4 << 20;
    let mut sink = Sink {
        w,
        index: IndexBuilder::default(),
        written: 0,
    };
    if format.bom {
        sink.put(bom_bytes(format.encoding))?;
    }
    let raw_ok = origin.is_some_and(|o| o.raw_copy(format));
    let mut eol = EolConv {
        target: format.line_ending,
        pending_cr: false,
    };
    let mut enc = Enc::new(format.encoding, on_unmappable);
    let mut raw_copied = 0u64;
    let mut converted = Vec::with_capacity(BLOCK as usize * 2);
    let mut encoded = Vec::with_capacity(BLOCK as usize * 2);
    let mut pieces = Vec::new();
    let _ = snap.tree.visit(0, u64::MAX, &mut |ps, p| {
        pieces.push((ps, *p));
        ControlFlow::Continue(())
    });
    let len = snap.len();
    for (ps, p) in pieces {
        if raw_ok && p.buf == ORIGINAL {
            let o = origin.expect("raw_ok implies an origin");
            let range = match &o.transcoded {
                None => p.start..p.end(),
                Some(t) => t.raw_range(p.start..p.end())?,
            };
            let mut at = range.start;
            let mut buf = vec![0u8; BLOCK.min(range.end - range.start) as usize];
            while at < range.end {
                let n = BLOCK.min(range.end - at) as usize;
                o.raw.read_at(at, &mut buf[..n])?;
                sink.put(&buf[..n])?;
                at += n as u64;
            }
            raw_copied += range.end - range.start;
            continue;
        }
        let mut result = Ok(());
        snap.for_each_block(ps..ps + p.len, BLOCK as usize, &mut |at, bytes| {
            converted.clear();
            eol.convert(bytes, &mut converted);
            let last = at + bytes.len() as u64 == len;
            if last {
                eol.finish(&mut converted);
            }
            encoded.clear();
            if let Err(e) = enc.encode(&converted, last, &mut encoded) {
                result = Err(e);
                return Ok(ControlFlow::Break(()));
            }
            sink.put(&encoded)?;
            Ok(ControlFlow::Continue(()))
        })?;
        result?;
    }
    // A pending CR or carried bytes at the very end (last piece was raw).
    converted.clear();
    eol.finish(&mut converted);
    encoded.clear();
    enc.encode(&converted, true, &mut encoded)?;
    sink.put(&encoded)?;
    if enc.unmappable > 0 && on_unmappable == OnUnmappable::Fail {
        return Err(SaveError::Unmappable {
            count: enc.unmappable,
            sample: enc.first_unmappable.map(String::from).unwrap_or_default(),
        });
    }
    let Sink { w, index, written } = sink;
    Ok((
        WriteStats {
            written,
            raw_copied,
            unmappable: enc.unmappable,
            first_unmappable: enc.first_unmappable,
            invalid: enc.invalid,
            index: index.finish(),
        },
        w,
    ))
}

/// Writes the document's bytes in `format` to any writer (export, tests).
pub fn write_to<W: Write>(
    w: W,
    file: &TextFile,
    format: &SaveFormat,
    on_unmappable: OnUnmappable,
) -> Result<u64, SaveError> {
    let snap = file.doc.snapshot();
    let (stats, _) = write_content(w, &snap, file.origin.as_ref(), format, on_unmappable)?;
    Ok(stats.written)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map_or_else(|| "untitled".into(), |n| n.to_string_lossy().into_owned());
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(".~{name}.{}.{n}.quire-tmp", std::process::id()))
}

/// Copies history pieces that live in `from` (an old original about to be
/// unmapped) into a memory buffer, newest steps first; steps that don't fit
/// in [`SPILL_MAX`] are dropped. With `keep_as`, the old buffer survives
/// under that id instead and nothing is copied. Returns steps dropped.
fn rehome_history(
    doc: &mut TextDocument,
    from: BufferId,
    keep_as: Option<BufferId>,
) -> io::Result<u64> {
    let buffers = doc.buffer().buffers().clone();
    let trees = |c: &Change| -> Vec<PieceTree> {
        match c {
            Change::Ops(ops) => ops
                .iter()
                .flat_map(|o| [o.removed.clone(), o.inserted.clone()])
                .collect(),
            Change::Tree { before, after } => vec![before.clone(), after.clone()],
            Change::Format { .. } => vec![],
        }
    };
    let refs = |t: &PieceTree| -> u64 {
        t.pieces()
            .iter()
            .filter(|p| p.buf == from)
            .map(|p| p.len)
            .sum()
    };
    if let Some(to) = keep_as {
        for tx in doc.history_mut().all_mut() {
            remap_change(&mut tx.change, &mut |p| {
                if p.buf == from {
                    Piece { buf: to, ..p }
                } else {
                    p
                }
            });
        }
        return Ok(0);
    }
    // Size per step, newest first: undo stack top is last.
    let sizes: Vec<u64> = doc
        .history()
        .all()
        .map(|tx| trees(&tx.change).iter().map(&refs).sum())
        .collect();
    let undo_len = doc.history().undo_len();
    let mut total = 0u64;
    let mut drop_oldest = 0usize;
    // Redo entries (after the undo ones) are newer than any undo entry.
    let mut order: Vec<usize> = (undo_len..sizes.len()).collect();
    order.extend((0..undo_len).rev());
    for i in order {
        total += sizes[i];
        if total > SPILL_MAX && i < undo_len {
            drop_oldest = i + 1;
            break;
        }
    }
    let trimmed = drop_oldest as u64;
    doc.history_mut().drop_oldest(drop_oldest);
    let src = buffers.get(from).source;
    let mut spill = Vec::new();
    let mut map = std::collections::HashMap::new();
    for tx in doc.history().all() {
        for t in trees(&tx.change) {
            for p in t.pieces() {
                if p.buf == from && !map.contains_key(&(p.start, p.len)) {
                    let at = spill.len() as u64;
                    spill.extend_from_slice(&src.read_vec(p.start, p.len as usize)?);
                    map.insert((p.start, p.len), at);
                }
            }
        }
    }
    if map.is_empty() {
        return Ok(trimmed);
    }
    let source: Arc<dyn Source> = Arc::new(MemorySource::new(spill));
    let index = Arc::new(ChunkIndex::build(&*source)?);
    let to = buffers.push(BufferEntry { source, index });
    for tx in doc.history_mut().all_mut() {
        remap_change(&mut tx.change, &mut |p| {
            if p.buf == from {
                Piece {
                    buf: to,
                    start: map[&(p.start, p.len)],
                    ..p
                }
            } else {
                p
            }
        });
    }
    Ok(trimmed)
}

fn remap_change(c: &mut Change, f: &mut dyn FnMut(Piece) -> Piece) {
    let mut remap = |t: &mut PieceTree| {
        if t.pieces().iter().any(|p| f(*p) != *p) {
            let pieces: Vec<Piece> = t.pieces().into_iter().map(&mut *f).collect();
            *t = PieceTree::from_pieces(&pieces);
        }
    };
    match c {
        Change::Ops(ops) => {
            for o in ops {
                remap(&mut o.removed);
                remap(&mut o.inserted);
            }
        }
        Change::Tree { before, after } => {
            remap(before);
            remap(after);
        }
        Change::Format { .. } => {}
    }
}

/// Atomically replaces `target` with `temp`.
fn replace_file(target: &Path, temp: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
        };
        let t = verbatim(target);
        let s = verbatim(temp);
        let exists = target.exists();
        let mut last = io::Error::other("replace failed");
        // Another process (an indexer, an antivirus) may hold the file
        // briefly: retry for about a second.
        for _ in 0..10 {
            // SAFETY: both strings are NUL-terminated wide paths.
            let ok = unsafe {
                if exists {
                    ReplaceFileW(
                        t.as_ptr(),
                        s.as_ptr(),
                        std::ptr::null(),
                        REPLACEFILE_IGNORE_MERGE_ERRORS,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                } else {
                    MoveFileExW(
                        s.as_ptr(),
                        t.as_ptr(),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                }
            };
            if ok != 0 {
                return Ok(());
            }
            last = io::Error::last_os_error();
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(last)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if let Ok(meta) = std::fs::metadata(target) {
            std::fs::set_permissions(temp, std::fs::Permissions::from_mode(meta.mode()))?;
            if let Ok(f) = File::open(temp) {
                use std::os::fd::AsRawFd;
                // Best effort: only root may give a file away.
                // SAFETY: valid descriptor; failure is ignored.
                unsafe {
                    libc::fchown(f.as_raw_fd(), meta.uid(), meta.gid());
                }
            }
        }
        std::fs::rename(temp, target)?;
        if let Some(dir) = target.parent() {
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

/// `\\?\` form of an absolute path, NUL-terminated, so paths over 260
/// characters work even where long paths aren't enabled (§9).
#[cfg(windows)]
fn verbatim(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.as_os_str().to_string_lossy();
    let v = if s.starts_with(r"\\?\") {
        s.into_owned()
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{s}")
    };
    std::ffi::OsStr::new(&v)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[derive(Debug)]
pub struct SaveOptions<'a> {
    pub on_unmappable: OnUnmappable,
    /// Re-decoding a legacy file after the save (to keep raw copies exact).
    pub threads: usize,
    pub temp_dir: &'a Path,
}

/// Saves `file` to `target` (its own path when `None`). See the module docs.
pub fn save(
    file: &mut TextFile,
    target: Option<&Path>,
    opts: &SaveOptions<'_>,
) -> Result<SaveReport, SaveError> {
    let started = Instant::now();
    let target = match target.or(file.path.as_deref()) {
        Some(t) => t.to_path_buf(),
        None => {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no path to save to").into());
        }
    };
    let same_file = file.path.as_deref().is_some_and(|p| same_path(p, &target));
    let format = file.doc.format;
    let snap = file.doc.snapshot();

    // 1. Stream into a temp file next to the target, then fsync it.
    let temp = temp_path(&target);
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = write_content(
        BufWriter::with_capacity(8 << 20, out),
        &snap,
        file.origin.as_ref(),
        &format,
        opts.on_unmappable,
    )
    .and_then(|(stats, w)| {
        let f = w.into_inner().map_err(io::IntoInnerError::into_error)?;
        f.sync_all()?;
        Ok(stats)
    });
    let stats = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    };
    let changed_text =
        format.line_ending.is_some() || (stats.unmappable > 0) || (stats.invalid > 0);

    // 2. Undo ranges in the old original move out of the mapping (or, when
    //    saving elsewhere or for transcoded files, keep their buffer).
    let buffers = file.doc.buffer().buffers().clone();
    let old0 = buffers.get(ORIGINAL);
    let keep_old = !same_file || file.origin.as_ref().is_some_and(|o| o.transcoded.is_some());
    let keep_as = keep_old.then(|| buffers.push(old0.clone()));
    let mut trimmed = rehome_history(&mut file.doc, ORIGINAL, keep_as)?;
    if changed_text {
        // The saved text differs from the document; old offsets are void.
        trimmed += file.doc.history().undo_len() as u64;
        file.doc.history_mut().clear();
    }

    // 3. Unmap the original: drop every reference we hold to it.
    let utf8_original = file.origin.as_ref().is_some_and(|o| o.transcoded.is_none());
    let old_origin = if same_file { file.origin.take() } else { None };
    let old_index = Arc::clone(&old0.index);
    if same_file && utf8_original {
        let empty: Arc<dyn Source> = Arc::new(MemorySource::new(Vec::new()));
        buffers.replace(
            ORIGINAL,
            BufferEntry {
                source: empty,
                index: Arc::new(ChunkIndex::unindexed(0)),
            },
        );
    }
    drop(old0);
    drop(snap);

    // 4. Replace.
    if let Err(e) = replace_file(&target, &temp) {
        let _ = std::fs::remove_file(&temp);
        if same_file {
            restore_original(file, &target, old_origin, old_index)?;
        }
        return Err(e.into());
    }

    // 5. Map the new file and re-point the document at it.
    let opened = source::open(&target, &file.governor, file.doc_id)?;
    let bom = if format.bom {
        bom_bytes(format.encoding).len() as u64
    } else {
        0
    };
    let index = Arc::new(stats.index);
    let transcoded = if format.encoding == UTF_8 {
        file.doc.buffer_mut().rebase(
            BufferEntry {
                source: Arc::clone(&opened.source),
                index,
            },
            bom,
        );
        None
    } else {
        let t = Arc::new(transcode(
            Arc::clone(&opened.source),
            format.encoding,
            bom,
            opts.temp_dir,
            opts.threads,
            &|_| true,
        )?);
        let idx = Arc::new(ChunkIndex::unindexed(t.utf8.len()));
        t.utf8.prefetch(0, t.utf8.len());
        idx.scan_range(&*t.utf8, 0, idx.chunk_count(), None)?;
        file.doc.buffer_mut().rebase(
            BufferEntry {
                source: Arc::clone(&t.utf8),
                index: idx,
            },
            0,
        );
        Some(t)
    };
    file.origin = Some(Origin {
        raw: opened.source,
        encoding: format.encoding,
        bom_len: bom,
        transcoded,
    });
    file.stamp = Some(opened.stamp);
    file.source_kind = opened.kind;
    file.remote = opened.remote;
    file.path = Some(target.clone());
    if let Some(le) = format.line_ending {
        file.doc.eol = le;
    }
    file.doc.format = SaveFormat {
        line_ending: None,
        ..format
    };
    let len = file.doc.len();
    let sels: Vec<_> = file
        .doc
        .selections()
        .iter()
        .map(|s| crate::document::Selection::new(s.anchor.min(len), s.head.min(len)))
        .collect();
    file.doc.set_selections(sels, usize::MAX);
    file.doc.mark_saved();

    let mut losses = Vec::new();
    if stats.unmappable > 0 {
        losses.push(format!(
            "{} characters not representable in {} were written as '?' (first: {:?})",
            stats.unmappable,
            format.encoding.name(),
            stats.first_unmappable.unwrap_or('?')
        ));
    }
    if stats.invalid > 0 {
        losses.push(format!(
            "{} invalid bytes were written as U+FFFD in {}",
            stats.invalid,
            format.encoding.name()
        ));
    }
    Ok(SaveReport {
        path: target.to_string_lossy().into_owned(),
        bytes_written: stats.written,
        raw_copied: stats.raw_copied,
        encoding: format.encoding.name().to_owned(),
        unmappable: stats.unmappable,
        invalid_bytes: stats.invalid,
        losses,
        history_trimmed: trimmed,
        millis: started.elapsed().as_millis() as u64,
    })
}

/// After a failed replace: map the untouched original again.
fn restore_original(
    file: &mut TextFile,
    path: &Path,
    old: Option<Origin>,
    index: Arc<ChunkIndex>,
) -> Result<(), SaveError> {
    let opened = source::open(path, &file.governor, file.doc_id)?;
    let buffers = file.doc.buffer().buffers().clone();
    match old {
        Some(o) if o.transcoded.is_some() => {
            // Buffer 0 (the decoded text) is ours and was kept; re-map raw.
            file.origin = Some(Origin {
                raw: opened.source,
                ..o
            });
        }
        Some(o) => {
            // Unchanged content: the old index still applies.
            buffers.replace(
                ORIGINAL,
                BufferEntry {
                    source: Arc::clone(&opened.source),
                    index,
                },
            );
            file.doc.buffer_mut().refresh_index();
            file.origin = Some(Origin {
                raw: opened.source,
                ..o
            });
        }
        None => {}
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests;
