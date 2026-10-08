use std::io;
use std::sync::Arc;

use super::{Source, SourceKind, check_range};

/// Bytes held in memory: empty files, transcoded small files, tests.
#[derive(Clone)]
pub struct MemorySource {
    bytes: Arc<[u8]>,
}

impl std::fmt::Debug for MemorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorySource")
            .field("len", &self.bytes.len())
            .finish()
    }
}

impl MemorySource {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Source for MemorySource {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(offset, buf.len(), self.len())?;
        let at = offset as usize;
        buf.copy_from_slice(&self.bytes[at..at + buf.len()]);
        Ok(())
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Memory
    }
}
