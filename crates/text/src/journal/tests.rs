use proptest::prelude::*;
use quire_core::memory::{GovernorConfig, MemoryGovernor};

use super::*;
use crate::document::Action;

fn header(path: Option<PathBuf>) -> Header {
    Header {
        stamp: path.as_ref().map(|p| FileStamp::of_path(p).unwrap()),
        path,
        encoding: "UTF-8".into(),
        created_ms: now_ms(),
        pid: std::process::id(),
    }
}

fn samples() -> Vec<Record> {
    vec![
        Record::Edit {
            ops: vec![(5, 2, b"abc".to_vec()), (0, 0, Vec::new())],
            kind: 1,
            merged: true,
            selections: vec![(1, 2), (8, 8)],
        },
        Record::Undo,
        Record::Redo,
        Record::Format {
            encoding: "Shift_JIS".into(),
            bom: false,
            line_ending: Some(LineEnding::Crlf),
        },
        Record::ReplaceAll {
            query: Query {
                pattern: "a+".into(),
                regex: true,
                case_sensitive: false,
                whole_word: true,
            },
            replacement: "$0!".into(),
        },
    ]
}

#[test]
fn records_round_trip() {
    for r in samples() {
        let (back, t) = decode_record(&encode_record(&r, 42)).unwrap();
        assert_eq!((back, t), (r, 42));
    }
}

#[test]
fn torn_tails_lose_only_the_last_record() {
    let mut bytes = header_bytes(&header(None));
    let head = bytes.len();
    let mut ends = Vec::new();
    for r in samples() {
        bytes.extend_from_slice(&frame(&encode_record(&r, 1)));
        ends.push(bytes.len());
    }
    for cut in head..=bytes.len() {
        let j = parse(&bytes[..cut]).unwrap();
        let complete = ends.iter().filter(|&&e| e <= cut).count();
        assert_eq!(j.records.len(), complete, "cut at {cut}");
    }
    // A flipped byte stops reading there.
    let mut bad = bytes.clone();
    bad[ends[1] + 10] ^= 0xFF;
    assert_eq!(parse(&bad).unwrap().records.len(), 2);
    assert!(parse(b"QJRNL00").is_none());
}

proptest! {
    #[test]
    fn hostile_bytes_never_panic(tail in prop::collection::vec(any::<u8>(), 0..2000)) {
        let mut bytes = header_bytes(&header(None));
        bytes.extend_from_slice(&tail);
        let _ = parse(&bytes);
        let _ = decode_record(&tail);
        let _ = parse(&tail);
    }
}

#[test]
fn writer_appends_syncs_rarely_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("doc.txt");
    std::fs::write(&file, b"hello world\n").unwrap();
    let jdir = dir.path().join("journal");
    let jpath = jdir.join(journal_name(Some(&file)));
    let g = MemoryGovernor::new(GovernorConfig::default());

    let mut doc = crate::file::open(
        &file,
        &g,
        None,
        &crate::file::OpenSettings {
            encoding: None,
            temp_dir: dir.path().join("tmp"),
            threads: 1,
            progress: &|_| true,
        },
    )
    .unwrap();
    let w = JournalWriter::create(&jpath, &header(Some(file.clone()))).unwrap();
    // A live writer's journal is not an orphan.
    assert!(orphans(&jdir).is_empty());
    let started = Instant::now();
    for c in "typing fast".chars() {
        doc.doc
            .apply(&Action::Type {
                text: c.to_string(),
            })
            .unwrap();
        w.append(doc.doc.take_records());
    }
    doc.doc.apply(&Action::Undo).unwrap();
    doc.doc.apply(&Action::SelectAll).unwrap();
    doc.doc.apply(&Action::Type { text: "X".into() }).unwrap();
    w.append(doc.doc.take_records());
    let stats = Arc::clone(&w.stats);
    w.close(false);
    let syncs = stats.syncs.load(Ordering::Relaxed);
    // At most one fsync per second, plus the one on close.
    assert!(syncs <= started.elapsed().as_secs() + 2, "{syncs} syncs");

    let found = orphans(&jdir);
    assert_eq!(found.len(), 1);
    let j = read(&found[0].journal).unwrap();
    let back = recover(&j, &g, &dir.path().join("tmp"), 1).unwrap();
    assert_eq!(back.doc.text(0, back.doc.len()).unwrap(), "X");
    assert!(back.doc.is_dirty());
    // Undo history comes back too.
    let mut back = back;
    back.doc.apply(&Action::Undo).unwrap();
    back.doc.apply(&Action::Redo).unwrap();
    back.doc.apply(&Action::Undo).unwrap();
    back.doc.apply(&Action::Undo).unwrap();
    assert_eq!(back.doc.text(0, back.doc.len()).unwrap(), "hello world\n");

    // A changed file refuses recovery. (Windows refuses to truncate a file
    // we have mapped, so let go of it first.)
    drop(back);
    drop(doc);
    std::fs::write(&file, b"changed by someone else").unwrap();
    assert!(matches!(
        recover(&j, &g, dir.path(), 1),
        Err(RecoverError::FileChanged)
    ));
}

#[test]
fn clean_close_deletes_and_reset_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.qj");
    let w = JournalWriter::create(&p, &header(None)).unwrap();
    w.append(vec![Record::Undo]);
    w.reset(header(None));
    w.append(vec![Record::Redo]);
    w.close(false);
    let j = read(&p).unwrap();
    assert_eq!(j.records.len(), 1);
    assert_eq!(j.records[0].0, Record::Redo);
    let w = JournalWriter::create(&p, &header(None)).unwrap();
    w.close(true);
    assert!(!p.exists());
}
