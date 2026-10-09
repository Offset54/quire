//! Seed inputs for the fuzz targets in `fuzz/` (task 13):
//! `cargo run -p quire-text --example fuzzseeds -- fuzz/seeds`.
//! A valid journal is out of a fuzzer's reach by chance (magic and
//! checksums), so it starts from real ones.

use std::path::Path;

use quire_text::document::{Action, TextDocument};
use quire_text::journal::{Header, JournalWriter, now_ms};

fn put(dir: &Path, target: &str, name: &str, bytes: &[u8]) {
    let d = dir.join(target);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(name), bytes).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));

    // encoding: [selector][step][raw bytes]
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("日本語のテキスト、ｶﾀｶﾅ 123\r\n表示");
    let (euckr, _, _) = encoding_rs::EUC_KR.encode("한국어 텍스트, 줄 2\n끝");
    let utf16: Vec<u8> = "UTF-16 😀 漢字\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    for (i, (sel, raw)) in [
        (0u8, &*sjis),
        (1, &*euckr),
        (6, &utf16[..]),
        (8, b"caf\xe9 na\xefve"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut v = vec![sel, 3];
        v.extend_from_slice(raw);
        put(&out, "encoding", &format!("seed-{i}"), &v);
    }

    // piece_tree: [initial length][op][a][b]...
    put(
        &out,
        "piece_tree",
        "seed-0",
        &[8, 1, 2, 3, 4, 5, 6, 7, 0, 3, 1, 2, 9, 4, 0, 6, 2, 3],
    );

    // search: [flags][pattern length][pattern][document]
    put(&out, "search", "seed-0", b"\x00\x01abab ab\nba abab");
    put(&out, "search", "seed-1", b"\x05\x03a.bacb a b axb\r\naab");
    put(
        &out,
        "search",
        "seed-2",
        "\x02\x05straßeSTRASSE Straße strasse".as_bytes(),
    );

    // journal: real journals from typing, deleting, undo and redo.
    let tmp = std::env::temp_dir().join(format!("quire-fuzzseeds-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut doc = TextDocument::from_bytes(b"line one\r\nline two\nthree");
    let actions = [
        Action::Type {
            text: "hello".into(),
        },
        Action::Newline,
        Action::Backspace,
        Action::Undo,
        Action::Redo,
        Action::Type {
            text: "한글\r\n".into(),
        },
    ];
    let jpath = tmp.join("seed.journal");
    let w = JournalWriter::create(
        &jpath,
        &Header {
            path: None,
            stamp: None,
            encoding: "UTF-8".into(),
            created_ms: now_ms(),
            pid: std::process::id(),
        },
    )
    .unwrap();
    for a in &actions {
        doc.apply(a).unwrap();
        w.append(doc.take_records());
    }
    w.close(false);
    let bytes = std::fs::read(&jpath).unwrap();
    put(&out, "journal", "seed-0", &bytes);
    // A torn tail.
    put(&out, "journal", "seed-1", &bytes[..bytes.len() - 3]);
    std::fs::remove_dir_all(&tmp).unwrap();
}
