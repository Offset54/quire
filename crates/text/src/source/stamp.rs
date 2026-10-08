//! A file's identity and version: what external change detection compares
//! (§5.8) and what the recovery journal checks before replaying (§5.7).

use std::fs::File;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStamp {
    pub len: u64,
    /// Last write time, nanoseconds since the Unix epoch.
    pub modified_ns: u64,
    /// Volume and file index (Windows) or device and inode (Unix): changes
    /// when another program replaces the file instead of writing in place.
    pub file_id: Option<(u64, u64)>,
}

impl FileStamp {
    pub fn of(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        Ok(Self {
            len: meta.len(),
            modified_ns,
            file_id: file_id(file, &meta),
        })
    }

    pub fn of_path(path: &Path) -> io::Result<Self> {
        Self::of(&File::open(path)?)
    }
}

#[cfg(windows)]
fn file_id(file: &File, _: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: zeroed is a valid out-parameter; the handle is open.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    (ok != 0).then(|| {
        (
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    })
}

#[cfg(unix)]
fn file_id(_: &File, meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(any(windows, unix)))]
fn file_id(_: &File, _: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}
