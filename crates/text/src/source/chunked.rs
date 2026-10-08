//! Positional reads through a governed LRU chunk cache, for files where a
//! mapping is unsafe: network drives and cloud placeholders (§5.5).

use std::fs::File;
use std::io;
use std::sync::Arc;

use quire_core::memory::{CacheClass, DocId, LruCache, MemoryGovernor};

use super::{Source, SourceKind, check_range};

/// Cache granularity.
pub const CHUNK: u64 = 256 * 1024;

pub struct ChunkedSource {
    file: File,
    len: u64,
    cache: Arc<LruCache<u64, Arc<[u8]>>>,
}

impl std::fmt::Debug for ChunkedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkedSource")
            .field("len", &self.len)
            .field("cached", &self.cache.len())
            .finish()
    }
}

impl ChunkedSource {
    pub fn new(file: File, len: u64, governor: &MemoryGovernor, doc: Option<DocId>) -> Self {
        Self {
            file,
            len,
            cache: LruCache::new(governor, "source-chunks", doc, CacheClass::Speculative),
        }
    }

    fn chunk(&self, index: u64) -> io::Result<Arc<[u8]>> {
        if let Some(c) = self.cache.get(&index) {
            return Ok(c);
        }
        let start = index * CHUNK;
        let len = CHUNK.min(self.len - start) as usize;
        let mut buf = vec![0u8; len];
        read_exact_at(&self.file, start, &mut buf)?;
        let chunk: Arc<[u8]> = buf.into();
        self.cache.insert(index, Arc::clone(&chunk), len as u64);
        Ok(chunk)
    }
}

fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, buf, offset)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file shrank while reading",
            ));
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

impl Source for ChunkedSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(offset, buf.len(), self.len)?;
        // Large reads (scans, saves) bypass the cache so they don't flush it.
        if buf.len() as u64 > 4 * CHUNK {
            return read_exact_at(&self.file, offset, buf);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let at = offset + done as u64;
            let chunk = self.chunk(at / CHUNK)?;
            let within = (at % CHUNK) as usize;
            let n = (chunk.len() - within).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&chunk[within..within + n]);
            done += n;
        }
        Ok(())
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Chunked
    }
}
