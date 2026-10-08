//! Memory-mapped files with guarded reads (ADR-0007).

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io;

use memmap2::Mmap;

use super::{Source, SourceKind, check_range};

unsafe extern "C" {
    fn quire_guarded_copy(dst: *mut c_void, src: *const c_void, len: usize) -> c_int;
    fn quire_guard_selftest() -> c_int;
}

/// Raises the platform's mapped-I/O fault inside the guard and reports
/// whether the guard caught it. A real fault needs a failing device, so
/// tests use this on Windows (Unix tests also truncate a mapped file).
pub fn guard_selftest() -> bool {
    // SAFETY: the self-test only raises and catches a fault on this thread.
    unsafe { quire_guard_selftest() == 1 }
}

/// A read-only mapping of a whole file.
pub struct MappedSource {
    map: Mmap,
    /// Kept open for the mapping's lifetime.
    _file: File,
}

impl std::fmt::Debug for MappedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedSource")
            .field("len", &self.map.len())
            .finish()
    }
}

impl MappedSource {
    /// Maps `file`, which must not be empty (empty files can't be mapped).
    pub fn new(file: File) -> io::Result<Self> {
        // SAFETY: the mapping is only ever read through `read_at`, which
        // copies out under the I/O-error guard; no references into it escape.
        // Another process changing the file underneath us changes the bytes
        // we read, which external change detection reports (§5.8).
        let map = unsafe { Mmap::map(&file)? };
        Ok(Self { map, _file: file })
    }
}

impl Source for MappedSource {
    fn len(&self) -> u64 {
        self.map.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(offset, buf.len(), self.len())?;
        if buf.is_empty() {
            return Ok(());
        }
        // SAFETY: the range was checked against the mapping's length, and
        // `buf` is a distinct, writable allocation of `buf.len()` bytes.
        let faulted = unsafe {
            quire_guarded_copy(
                buf.as_mut_ptr().cast(),
                self.map.as_ptr().add(offset as usize).cast(),
                buf.len(),
            )
        };
        if faulted != 0 {
            return Err(io::Error::other(format!(
                "I/O error reading the mapped file at offset {offset}"
            )));
        }
        Ok(())
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Mapped
    }

    fn prefetch(&self, offset: u64, len: u64) {
        let total = self.len();
        if offset >= total {
            return;
        }
        let len = len.min(total - offset) as usize;
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Memory::{
                PrefetchVirtualMemory, WIN32_MEMORY_RANGE_ENTRY,
            };
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            let entry = WIN32_MEMORY_RANGE_ENTRY {
                // SAFETY: in bounds, checked above.
                VirtualAddress: unsafe { self.map.as_ptr().add(offset as usize) } as *mut c_void,
                NumberOfBytes: len,
            };
            // SAFETY: a hint over a range of our own mapping; failure is harmless.
            unsafe {
                PrefetchVirtualMemory(GetCurrentProcess(), 1, &entry, 0);
            }
        }
        #[cfg(unix)]
        {
            let _ = self
                .map
                .advise_range(memmap2::Advice::WillNeed, offset as usize, len);
        }
    }
}
