//! External change detection (§5.8): what happened to the file on disk, and
//! whether "keep mine" is still possible.
//!
//! A cheap stamp check (length, last-write time, file id) runs when the
//! watcher fires or the window gets focus. If the stamp moved, the content
//! is verified against the per-chunk fingerprints the line index recorded:
//! - only the time changed: nothing to do;
//! - bytes appended after an intact original: the document is unaffected;
//! - the file was replaced (new file id, as editors that save atomically
//!   do): our mapping still holds the old bytes, so keeping our version is
//!   safe;
//! - bytes changed in place: the mapping now shows the other program's
//!   bytes, so a document still referencing them can't be kept as it was.
//!   (While a document has unsaved edits we hold a handle that denies
//!   write sharing, so other programs can't do this on Windows.)

use std::fs::File;
use std::io;
use std::path::Path;

use serde::Serialize;
use xxhash_rust::xxh3::xxh3_64;

use crate::buffer::ORIGINAL;
use crate::file::TextFile;
use crate::index::CHUNK;
use crate::source::FileStamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiskChange {
    /// Only the timestamp moved; the bytes are the same.
    Touched,
    /// Bytes were added after the unchanged original.
    Appended,
    /// Bytes we read were changed in place.
    Modified,
    /// Another file now has this name.
    Replaced,
    Deleted,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStatus {
    pub change: DiskChange,
    /// Our version can still be kept and saved over the disk version.
    pub keep_mine_possible: bool,
    pub stamp: Option<FileStamp>,
}

/// The file's current stamp if it differs from the one we know.
/// `Ok(Some(None))`: the file is gone.
pub fn stamp_changed(file: &TextFile) -> io::Result<Option<Option<FileStamp>>> {
    let (Some(path), Some(known)) = (&file.path, &file.stamp) else {
        return Ok(None);
    };
    match FileStamp::of_path(path) {
        Ok(now) if now == *known => Ok(None),
        Ok(now) => Ok(Some(Some(now))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Some(None)),
        Err(e) => Err(e),
    }
}

/// Works out what changed, reading the original to verify fingerprints
/// (run it off the UI thread for large files).
pub fn classify(file: &TextFile, now: Option<FileStamp>) -> io::Result<DiskStatus> {
    let Some(known) = file.stamp else {
        return Ok(DiskStatus {
            change: DiskChange::Touched,
            keep_mine_possible: true,
            stamp: now,
        });
    };
    let Some(now) = now else {
        return Ok(DiskStatus {
            change: DiskChange::Deleted,
            keep_mine_possible: true,
            stamp: None,
        });
    };
    if now.file_id != known.file_id {
        return Ok(DiskStatus {
            change: DiskChange::Replaced,
            keep_mine_possible: true,
            stamp: Some(now),
        });
    }
    let transcoded = file.origin.as_ref().is_some_and(|o| o.transcoded.is_some());
    if transcoded {
        // Our decoded text is our own copy; only raw copies are affected.
        return Ok(DiskStatus {
            change: DiskChange::Modified,
            keep_mine_possible: true,
            stamp: Some(now),
        });
    }
    let intact = original_intact(file)?;
    let change = match (intact, now.len > known.len) {
        (true, true) => DiskChange::Appended,
        (true, false) if now.len == known.len => DiskChange::Touched,
        _ => DiskChange::Modified,
    };
    Ok(DiskStatus {
        change,
        keep_mine_possible: intact,
        stamp: Some(now),
    })
}

/// Re-hashes every indexed chunk of the original and compares it with the
/// fingerprint taken when it was indexed. A read error (the file shrank
/// under the mapping) counts as changed.
fn original_intact(file: &TextFile) -> io::Result<bool> {
    let e = file.doc.buffer().buffers().get(ORIGINAL);
    let len = e.source.len();
    let mut buf = vec![0u8; CHUNK as usize];
    for i in 0..e.index.chunk_count() {
        let Some(fp) = e.index.fingerprint(i) else {
            continue;
        };
        let start = i as u64 * CHUNK;
        let n = CHUNK.min(len - start) as usize;
        if e.source.read_at(start, &mut buf[..n]).is_err() || xxh3_64(&buf[..n]) != fp {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Accepts the disk change for "keep mine": remember the new stamp so the
/// user isn't asked again, and stop copying raw bytes that may now differ.
pub fn keep_mine(file: &mut TextFile, status: &DiskStatus) {
    file.stamp = status.stamp;
    if status.change == DiskChange::Modified
        && let Some(o) = &mut file.origin
    {
        o.raw_intact = false;
    }
}

/// One difference between our text and the disk's, by lines.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub mine_lines: (usize, usize),
    pub disk_lines: (usize, usize),
    pub mine: String,
    pub disk: String,
}

/// Compare is offered up to this size (each side).
pub const COMPARE_MAX: u64 = 10 << 20;

/// Line differences between the document and the file on disk, decoded
/// with the document's encoding. `None` when either side is larger than
/// [`COMPARE_MAX`] (the UI says so).
pub fn compare(file: &TextFile) -> io::Result<Option<Vec<Hunk>>> {
    let Some(path) = &file.path else {
        return Ok(None);
    };
    let disk_len = std::fs::metadata(path)?.len();
    if disk_len > COMPARE_MAX || file.doc.len() > COMPARE_MAX {
        return Ok(None);
    }
    let raw = std::fs::read(path)?;
    let (disk, _, _) = file.doc.format.encoding.decode(&raw);
    let mine = file.doc.text(0, file.doc.len())?;
    let diff = similar::TextDiff::from_lines(mine.as_str(), &*disk);
    let mut hunks = Vec::new();
    for op in diff.ops() {
        if op.tag() == similar::DiffTag::Equal {
            continue;
        }
        let (o, n) = (op.old_range(), op.new_range());
        let take = |lines: &[&str]| -> String { lines.concat() };
        hunks.push(Hunk {
            mine_lines: (o.start, o.end),
            disk_lines: (n.start, n.end),
            mine: take(&diff.old_slices()[o.clone()]),
            disk: take(&diff.new_slices()[n.clone()]),
        });
    }
    Ok(Some(hunks))
}

/// A handle that denies other programs write access to the file while we
/// hold unsaved edits that reference its bytes. On Unix, locks are only
/// advisory, so this holds nothing.
#[derive(Debug)]
pub struct EditLock {
    _file: Option<File>,
}

impl EditLock {
    /// Fails if another program already has the file open for writing.
    pub fn acquire(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
            let f = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .open(path)?;
            Ok(Self { _file: Some(f) })
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            Ok(Self { _file: None })
        }
    }
}

#[cfg(test)]
mod tests;
