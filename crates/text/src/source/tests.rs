use std::fs::File;
use std::io::Write;

use quire_core::memory::GovernorConfig;

use super::*;

fn governor() -> MemoryGovernor {
    MemoryGovernor::new(GovernorConfig::default())
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn temp_file(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn memory_source_reads_and_checks_bounds() {
    let s: Arc<dyn Source> = Arc::new(MemorySource::new(pattern(100)));
    assert_eq!(s.read_vec(10, 5).unwrap(), pattern(100)[10..15]);
    assert!(s.read_vec(98, 5).is_err());
    assert!(s.read_vec(u64::MAX, 1).is_err());
    assert_eq!(s.read_vec(100, 0).unwrap(), Vec::<u8>::new());
}

#[test]
fn mapped_source_matches_the_file() {
    let data = pattern(3 * 65536 + 17);
    let f = temp_file(&data);
    let opened = open(f.path(), &governor(), None).unwrap();
    assert_eq!(opened.kind, SourceKind::Mapped);
    assert_eq!(opened.stamp.len, data.len() as u64);
    let s = &opened.source;
    assert_eq!(s.read_vec(0, data.len()).unwrap(), data);
    assert_eq!(s.read_vec(65530, 20).unwrap(), data[65530..65550]);
    assert!(s.read_vec(data.len() as u64 - 1, 2).is_err());
    s.prefetch(0, u64::MAX);
}

#[test]
fn empty_files_open_as_memory() {
    let f = temp_file(b"");
    let opened = open(f.path(), &governor(), None).unwrap();
    assert_eq!(opened.kind, SourceKind::Memory);
    assert!(opened.source.is_empty());
}

#[test]
fn chunked_source_reads_across_chunks_and_is_governed() {
    let data = pattern(2 * chunked::CHUNK as usize + 1000);
    let f = temp_file(&data);
    let g = governor();
    let file = File::open(f.path()).unwrap();
    let s = ChunkedSource::new(file, data.len() as u64, &g, None);
    let at = chunked::CHUNK - 10;
    let mut buf = vec![0; 30];
    s.read_at(at, &mut buf).unwrap();
    assert_eq!(buf, data[at as usize..at as usize + 30]);
    assert_eq!(g.total_bytes(), 2 * chunked::CHUNK);
    // Large reads bypass the cache.
    let mut all = vec![0; data.len()];
    s.read_at(0, &mut all).unwrap();
    assert_eq!(all, data);
    assert!(s.read_at(data.len() as u64, &mut [0]).is_err());
}

#[test]
fn guard_catches_the_platform_fault() {
    assert!(guard_selftest());
}

/// A file truncated under a live mapping raises SIGBUS on access; the guard
/// must turn that into an error. (Windows refuses to truncate a mapped file.)
#[cfg(unix)]
#[test]
fn reading_a_truncated_mapping_is_an_error_not_a_crash() {
    let data = pattern(64 * 1024);
    let f = temp_file(&data);
    let opened = open(f.path(), &governor(), None).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(f.path())
        .unwrap()
        .set_len(0)
        .unwrap();
    let err = opened.source.read_vec(32 * 1024, 100).unwrap_err();
    assert!(err.to_string().contains("I/O error"), "{err}");
    // The guard is reusable after a fault.
    assert!(opened.source.read_vec(32 * 1024, 100).is_err());
}

#[test]
fn local_files_are_not_remote() {
    let f = temp_file(b"x");
    assert_eq!(remote_kind(f.path()), None);
}

#[cfg(windows)]
#[test]
fn unc_paths_are_network() {
    assert_eq!(
        remote_kind(Path::new(r"\\server\share\a.txt")),
        Some(RemoteKind::Network)
    );
    assert_eq!(
        remote_kind(Path::new(r"\\?\UNC\server\share\a.txt")),
        Some(RemoteKind::Network)
    );
}

#[test]
fn stamps_change_when_the_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.txt");
    std::fs::write(&p, b"one").unwrap();
    let a = FileStamp::of_path(&p).unwrap();
    let q = dir.path().join("b.txt");
    std::fs::write(&q, b"two").unwrap();
    std::fs::rename(&q, &p).unwrap();
    let b = FileStamp::of_path(&p).unwrap();
    assert_eq!(a.len, b.len);
    assert_ne!(a.file_id, b.file_id);
}
