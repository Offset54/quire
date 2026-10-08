use std::io::{Seek, SeekFrom, Write};

use quire_core::memory::{GovernorConfig, MemoryGovernor};

use super::*;
use crate::file::{OpenSettings, open};

fn opened(dir: &Path, name: &str, bytes: &[u8]) -> TextFile {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    let g = MemoryGovernor::new(GovernorConfig::default());
    let f = open(
        &p,
        &g,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: dir.join("tmp"),
            threads: 1,
            progress: &|_| true,
        },
    )
    .unwrap();
    let e = f.original_index();
    e.index
        .scan_range(&*e.source, 0, e.index.chunk_count(), None)
        .unwrap();
    f
}

fn status(f: &TextFile) -> Option<DiskStatus> {
    stamp_changed(f)
        .unwrap()
        .map(|now| classify(f, now).unwrap())
}

fn bump_mtime(p: &Path) {
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

#[test]
fn appends_touches_and_in_place_changes() {
    let dir = tempfile::tempdir().unwrap();
    let data = "line\n".repeat(30_000).into_bytes();
    let f = opened(dir.path(), "a.txt", &data);
    let p = f.path.clone().unwrap();
    assert!(status(&f).is_none());

    bump_mtime(&p);
    let s = status(&f).unwrap();
    assert_eq!(
        (s.change, s.keep_mine_possible),
        (DiskChange::Touched, true)
    );

    std::fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(b"more\n")
        .unwrap();
    let s = status(&f).unwrap();
    assert_eq!(
        (s.change, s.keep_mine_possible),
        (DiskChange::Appended, true)
    );

    let mut w = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    w.seek(SeekFrom::Start(70_000)).unwrap();
    w.write_all(b"XXXX").unwrap();
    drop(w);
    bump_mtime(&p);
    let s = status(&f).unwrap();
    assert_eq!(
        (s.change, s.keep_mine_possible),
        (DiskChange::Modified, false)
    );
}

#[test]
fn replaced_and_deleted_files_keep_our_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let f = opened(dir.path(), "b.txt", b"ours\n");
    let p = f.path.clone().unwrap();
    let other = dir.path().join("other.txt");
    std::fs::write(&other, b"theirs\n").unwrap();
    std::fs::rename(&other, &p).unwrap();
    let s = status(&f).unwrap();
    assert_eq!(
        (s.change, s.keep_mine_possible),
        (DiskChange::Replaced, true)
    );
    assert_eq!(f.doc.text(0, f.doc.len()).unwrap(), "ours\n");

    std::fs::remove_file(&p).unwrap();
    let s = status(&f).unwrap();
    assert_eq!(s.change, DiskChange::Deleted);
}

#[test]
fn compare_lists_changed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = opened(dir.path(), "c.txt", b"one\ntwo\nthree\n");
    f.doc
        .apply(&crate::document::Action::SetSelections {
            ranges: vec![(4, 7)],
        })
        .unwrap();
    f.doc
        .apply(&crate::document::Action::Type { text: "TWO".into() })
        .unwrap();
    let hunks = compare(&f).unwrap().unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].mine, "TWO\n");
    assert_eq!(hunks[0].disk, "two\n");
}

#[cfg(windows)]
#[test]
fn the_edit_lock_denies_other_writers() {
    let dir = tempfile::tempdir().unwrap();
    let f = opened(dir.path(), "d.txt", b"x");
    let p = f.path.clone().unwrap();
    let lock = EditLock::acquire(&p).unwrap();
    assert!(std::fs::OpenOptions::new().write(true).open(&p).is_err());
    // Readers are fine.
    assert!(File::open(&p).is_ok());
    drop(lock);
    assert!(std::fs::OpenOptions::new().write(true).open(&p).is_ok());
    // A writer already present makes the lock fail.
    let _w = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    assert!(EditLock::acquire(&p).is_err());
}
