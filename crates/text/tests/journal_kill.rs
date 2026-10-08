//! M1 DoD: killing the process mid-edit loses at most one second of work.
//!
//! The test re-runs its own binary as a child that types one character
//! every 2 ms into a document with a journal, reporting each keystroke on
//! stdout after it is applied. The parent kills the child (no chance to
//! flush or clean up), recovers the document from the journal, and checks
//! that every keystroke reported more than one second before the kill
//! survived. In practice nothing is lost: records reach the OS as soon as
//! they are committed.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::document::Action;
use quire_text::file::{OpenSettings, open};
use quire_text::journal::{self, Header, JournalWriter, now_ms};
use quire_text::source::FileStamp;

const CHILD_ENV: &str = "QUIRE_JOURNAL_KILL_CHILD";

fn settings(dir: &Path) -> OpenSettings<'static> {
    OpenSettings {
        encoding: None,
        temp_dir: dir.join("tmp"),
        threads: 1,
        progress: &|_| true,
    }
}

/// Child: open, journal, type forever.
#[test]
fn child_types_until_killed() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return; // not running as the child
    };
    let dir = Path::new(&dir);
    let path = dir.join("doc.txt");
    let g = MemoryGovernor::new(GovernorConfig::default());
    let mut f = open(&path, &g, None, &settings(dir)).unwrap();
    let w = JournalWriter::create(
        &dir.join("journal").join(journal::journal_name(Some(&path))),
        &Header {
            path: Some(path.clone()),
            stamp: Some(FileStamp::of_path(&path).unwrap()),
            encoding: "UTF-8".into(),
            created_ms: now_ms(),
            pid: std::process::id(),
        },
    )
    .unwrap();
    f.doc
        .apply(&Action::Move {
            to: quire_text::document::Move::DocEnd,
            extend: false,
        })
        .unwrap();
    // Bounded only for form: the parent kills it after a few seconds.
    for i in 0u64..1_000_000 {
        let c = char::from(b'a' + (i % 26) as u8);
        f.doc
            .apply(&Action::Type {
                text: c.to_string(),
            })
            .unwrap();
        w.append(f.doc.take_records());
        println!("{} {}", i + 1, now_ms());
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn killing_mid_edit_loses_at_most_one_second() {
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("doc.txt"), b"start:").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_types_until_killed",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let mut it = line.split_whitespace();
            if let (Some(n), Some(t)) = (it.next(), it.next())
                && let (Ok(n), Ok(t)) = (n.parse::<u64>(), t.parse::<u64>())
            {
                let _ = tx.send((n, t));
            }
        }
    });
    // Let it type for a while, then kill it without warning.
    let started = Instant::now();
    let mut reported = Vec::new();
    while started.elapsed() < Duration::from_millis(2500) {
        if let Ok(x) = rx.recv_timeout(Duration::from_millis(50)) {
            reported.push(x);
        }
    }
    child.kill().unwrap();
    let killed_at = now_ms();
    child.wait().unwrap();
    while let Ok(x) = rx.try_recv() {
        reported.push(x);
    }
    assert!(
        reported.len() > 100,
        "the child barely typed: {}",
        reported.len()
    );

    let orphans = journal::orphans(&dir.path().join("journal"));
    assert_eq!(
        orphans.len(),
        1,
        "the killed child's journal is recoverable"
    );
    let j = journal::read(&orphans[0].journal).unwrap();
    let g = MemoryGovernor::new(GovernorConfig::default());
    let back = journal::recover(&j, &g, &dir.path().join("tmp"), 1).unwrap();
    let text = back.doc.text(0, back.doc.len()).unwrap();
    let recovered = text.len() as u64 - "start:".len() as u64;

    // Everything typed more than a second before the kill must be back.
    let must_have = reported
        .iter()
        .filter(|&&(_, t)| t + 1000 <= killed_at)
        .map(|&(n, _)| n)
        .max()
        .unwrap_or(0);
    let last_reported = reported.last().map_or(0, |r| r.0);
    eprintln!(
        "typed {last_reported} (reported), recovered {recovered}, required {must_have}; lost {} keystrokes",
        last_reported.saturating_sub(recovered)
    );
    assert!(
        recovered >= must_have,
        "recovered {recovered} < {must_have}"
    );
    // The recovered text is exactly the typed prefix.
    let expected: String = (0..recovered)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    assert_eq!(&text["start:".len()..], expected);
}
