//! Byte sources behind a text document (MASTER_PROMPT §5.5; ADR-0007).
//!
//! - Local files are memory-mapped ([`MappedSource`]). Every read copies out
//!   of the mapping through a guard, so an I/O error behind the mapped view
//!   becomes an `io::Error` instead of a crash.
//! - Network drives and cloud placeholder files are read with positional
//!   reads into a governed LRU chunk cache ([`ChunkedSource`]).
//! - Small and synthetic content lives in memory ([`MemorySource`]).
//!
//! Sources never hand out borrowed slices of a mapping: callers copy the
//! range they need. The whole file is never copied into one buffer.

mod chunked;
mod detect;
mod mapped;
mod memory;
mod stamp;

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use quire_core::memory::{DocId, MemoryGovernor};
use serde::Serialize;

pub use chunked::ChunkedSource;
pub use detect::{RemoteKind, remote_kind};
pub use mapped::{MappedSource, guard_selftest};
pub use memory::MemorySource;
pub use stamp::FileStamp;

/// How a source reaches its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Mapped,
    Chunked,
    Memory,
}

/// Read-only random access to a fixed-length byte sequence.
pub trait Source: Send + Sync + fmt::Debug {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` with the bytes at `offset..offset + buf.len()`. Fails if
    /// the range runs past the end or the underlying read fails.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    fn kind(&self) -> SourceKind;

    /// Hint that `offset..offset + len` will be read soon.
    fn prefetch(&self, _offset: u64, _len: u64) {}
}

impl dyn Source {
    /// Reads `len` bytes at `offset` into a new vector.
    pub fn read_vec(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0; len];
        self.read_at(offset, &mut buf)?;
        Ok(buf)
    }
}

pub(crate) fn out_of_range(offset: u64, len: usize, total: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("read of {len} bytes at {offset} runs past the end ({total} bytes)"),
    )
}

pub(crate) fn check_range(offset: u64, len: usize, total: u64) -> io::Result<()> {
    match offset.checked_add(len as u64) {
        Some(end) if end <= total => Ok(()),
        _ => Err(out_of_range(offset, len, total)),
    }
}

/// An opened file and how it's being read.
#[derive(Debug, Clone)]
pub struct OpenedSource {
    pub source: Arc<dyn Source>,
    pub kind: SourceKind,
    /// Why the file is read in chunks instead of mapped, if it is.
    pub remote: Option<RemoteKind>,
    pub stamp: FileStamp,
}

/// Opens `path` for reading: mapped if local, chunked if it's on a network
/// drive or a cloud placeholder. `doc` is the document the chunk cache is
/// charged to.
pub fn open(
    path: &Path,
    governor: &MemoryGovernor,
    doc: Option<DocId>,
) -> io::Result<OpenedSource> {
    let file = File::open(path)?;
    let stamp = FileStamp::of(&file)?;
    let remote = remote_kind(path);
    let source: Arc<dyn Source> = if stamp.len == 0 {
        Arc::new(MemorySource::new(Vec::new()))
    } else if remote.is_some() {
        Arc::new(ChunkedSource::new(file, stamp.len, governor, doc))
    } else {
        Arc::new(MappedSource::new(file)?)
    };
    let kind = source.kind();
    Ok(OpenedSource {
        source,
        kind,
        remote,
        stamp,
    })
}

#[cfg(test)]
mod tests;
