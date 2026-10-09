use encoding_rs::{BIG5, EUC_KR, GB18030, SHIFT_JIS, WINDOWS_1251};
use quire_core::memory::GovernorConfig;

use super::*;
use crate::document::Action;
use crate::encoding::tests::{JA, KO, RU, ZH_HANS, ZH_HANT, encode};

fn governor() -> MemoryGovernor {
    MemoryGovernor::new(GovernorConfig::default())
}

fn open_bytes(dir: &Path, name: &str, bytes: &[u8]) -> TextFile {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    open_path(&p, None)
}

fn open_path(p: &Path, encoding: Option<&'static Encoding>) -> TextFile {
    let tmp = p.parent().unwrap().join("tmp");
    let opts = OpenSettings {
        encoding,
        temp_dir: tmp,
        threads: 2,
        progress: &|_| true,
    };
    let mut f = open(p, &governor(), None, &opts).unwrap();
    let e = f.original_index();
    e.index
        .scan_range(&*e.source, 0, e.index.chunk_count(), None)
        .unwrap();
    f.doc.buffer_mut().refresh_index();
    f
}

fn opts(dir: &Path) -> SaveOptions<'_> {
    SaveOptions {
        on_unmappable: OnUnmappable::Fail,
        threads: 2,
        temp_dir: dir,
    }
}

fn disk(f: &TextFile) -> Vec<u8> {
    std::fs::read(f.path.as_ref().unwrap()).unwrap()
}

fn text(f: &TextFile) -> String {
    f.doc.text(0, f.doc.len()).unwrap()
}

/// Inserts `s` after the first character at or past the middle.
fn edit_middle(f: &mut TextFile, s: &str) -> u64 {
    let t = text(f);
    let mid = (0..=t.len() / 2)
        .rev()
        .find(|&i| t.is_char_boundary(i))
        .unwrap() as u64;
    f.doc
        .apply(&Action::SetSelections {
            ranges: vec![(mid, mid)],
        })
        .unwrap();
    f.doc.apply(&Action::Type { text: s.into() }).unwrap();
    mid
}

fn samples() -> Vec<(&'static str, Vec<u8>)> {
    let mixed = "lf\ncrlf\r\ncr\rend".as_bytes().to_vec();
    let mut utf8_bom = b"\xEF\xBB\xBF".to_vec();
    utf8_bom.extend_from_slice("BOM 한국어\r\n".repeat(50).as_bytes());
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend(
        "UTF-16 😀 text\r\n"
            .repeat(400)
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    let mut sjis = encode(SHIFT_JIS, &JA.repeat(200));
    // NEC and IBM codes for the same character (Ⅰ): re-encoding would
    // collapse them, so only a raw copy keeps both.
    sjis.extend_from_slice(b"\x87\x54 \xFA\x4A\n");
    let mut bad_utf8 = "valid ".repeat(100).into_bytes();
    bad_utf8.extend_from_slice(b"\xFF\xFE\xC3 broken\n");
    vec![
        ("plain.txt", "hello\nworld\n".repeat(1000).into_bytes()),
        ("mixed.txt", mixed),
        ("bom.txt", utf8_bom),
        ("utf16.txt", utf16),
        ("cp949.txt", encode(EUC_KR, &KO.repeat(300))),
        ("sjis.txt", sjis),
        ("gb.txt", encode(GB18030, &ZH_HANS.repeat(300))),
        ("big5.txt", encode(BIG5, &ZH_HANT.repeat(300))),
        ("ru.txt", encode(WINDOWS_1251, &RU.repeat(300))),
        ("bad.txt", bad_utf8),
        ("empty.txt", Vec::new()),
    ]
}

#[test]
fn untouched_content_survives_save_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in samples() {
        let mut f = open_bytes(dir.path(), name, &bytes);
        // Unedited save: identical.
        let r = save(&mut f, None, &opts(dir.path())).unwrap();
        assert!(r.losses.is_empty(), "{name}: {:?}", r.losses);
        assert_eq!(disk(&f), bytes, "{name}: unedited save changed bytes");
        if bytes.is_empty() {
            continue;
        }
        // Edit in the middle: everything outside the edit is unchanged.
        let before = text(&f);
        let mid = edit_middle(&mut f, "XY");
        assert!(f.doc.is_dirty());
        let r = save(&mut f, None, &opts(dir.path())).unwrap();
        assert!(r.losses.is_empty(), "{name}: {:?}", r.losses);
        assert!(!f.doc.is_dirty());
        let after = disk(&f);
        let enc = f.doc.format.encoding;
        let inserted = {
            let mut e = Enc::new(enc, OnUnmappable::Fail);
            let mut out = Vec::new();
            e.encode(b"XY", true, &mut out).unwrap();
            out
        };
        // The raw split point: the original bytes before the edit decode to
        // the text before it.
        let prefix_len = (0..=bytes.len())
            .find(|&n| after[..n] != bytes[..n])
            .map_or(bytes.len(), |n| n - 1);
        assert_eq!(
            &after[prefix_len..prefix_len + inserted.len()],
            &inserted[..],
            "{name}"
        );
        assert_eq!(&after[..prefix_len], &bytes[..prefix_len], "{name}");
        assert_eq!(
            &after[prefix_len + inserted.len()..],
            &bytes[prefix_len..],
            "{name}"
        );
        assert_eq!(text(&f).replacen("XY", "", 1), before, "{name}");
        let _ = mid;

        // Undo after the save reaches into the old mapping (now in memory).
        f.doc.apply(&Action::Undo).unwrap();
        assert_eq!(text(&f), before, "{name}: undo after save");
        save(&mut f, None, &opts(dir.path())).unwrap();
        assert_eq!(disk(&f), bytes, "{name}: undo + save is the original");
    }
}

#[test]
fn line_endings_and_encodings_convert_and_losses_need_consent() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = open_bytes(dir.path(), "conv.txt", "a\nb\r\nc\rd 😀 한".as_bytes());
    f.doc.set_format(SaveFormat {
        line_ending: Some(LineEnding::Crlf),
        ..f.doc.format
    });
    save(&mut f, None, &opts(dir.path())).unwrap();
    assert_eq!(disk(&f), "a\r\nb\r\nc\r\nd 😀 한".as_bytes());
    assert_eq!(f.doc.snapshot().line_count(), Some(4));

    f.doc.set_format(SaveFormat {
        encoding: EUC_KR,
        bom: false,
        line_ending: None,
    });
    match save(&mut f, None, &opts(dir.path())) {
        Err(SaveError::Unmappable { count, sample }) => {
            assert_eq!(count, 1);
            assert_eq!(sample, "😀");
        }
        other => panic!("expected an unmappable error, got {other:?}"),
    }
    // Nothing was written.
    assert_eq!(disk(&f), "a\r\nb\r\nc\r\nd 😀 한".as_bytes());
    let r = save(
        &mut f,
        None,
        &SaveOptions {
            on_unmappable: OnUnmappable::Replace,
            ..opts(dir.path())
        },
    )
    .unwrap();
    assert_eq!(r.losses.len(), 1);
    assert_eq!(disk(&f), encode(EUC_KR, "a\r\nb\r\nc\r\nd ? 한"));
    assert_eq!(text(&f), "a\r\nb\r\nc\r\nd ? 한");

    // Back to UTF-8 with a BOM.
    f.doc.set_format(SaveFormat {
        encoding: UTF_8,
        bom: true,
        line_ending: Some(LineEnding::Lf),
    });
    save(&mut f, None, &opts(dir.path())).unwrap();
    assert_eq!(disk(&f), "\u{FEFF}a\nb\nc\nd ? 한".as_bytes());
}

#[test]
fn save_as_keeps_the_original_and_follows_the_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = open_bytes(dir.path(), "orig.txt", b"one\ntwo\n");
    edit_middle(&mut f, "!");
    let new = dir.path().join("copy.txt");
    save(&mut f, Some(&new), &opts(dir.path())).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("orig.txt")).unwrap(),
        b"one\ntwo\n"
    );
    assert_eq!(std::fs::read(&new).unwrap(), b"one\n!two\n");
    assert_eq!(f.path.as_deref(), Some(new.as_path()));
    f.doc.apply(&Action::Undo).unwrap();
    assert_eq!(text(&f), "one\ntwo\n");
}

#[test]
fn untitled_documents_save_to_a_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = TextFile::untitled(&governor(), dir.path().join("tmp"));
    f.doc.apply(&Action::Type { text: "new".into() }).unwrap();
    assert!(save(&mut f, None, &opts(dir.path())).is_err());
    let p = dir.path().join("new.txt");
    save(&mut f, Some(&p), &opts(dir.path())).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"new");
    // No temp files are left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("quire-tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

#[cfg(windows)]
#[test]
fn paths_longer_than_260_characters() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = dir.path().to_path_buf();
    for i in 0..12 {
        p.push(format!("a-rather-long-directory-name-{i}"));
    }
    std::fs::create_dir_all(&p).unwrap();
    let file = p.join("파일.txt");
    assert!(file.as_os_str().len() > 300);
    std::fs::write(&file, b"long path\n").unwrap();
    let mut f = open_path(&file, None);
    edit_middle(&mut f, "+");
    save(&mut f, None, &opts(dir.path())).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"long +path\n");
}

#[test]
fn threaded_writer_keeps_bytes_in_order_and_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("w.bin");
    let mut w = ThreadedWriter::new(File::create(&p).unwrap()).unwrap();
    let mut expected = Vec::new();
    // Writes smaller than, equal to and larger than a block, unaligned.
    for (i, n) in [
        1usize,
        7,
        WRITE_BLOCK - 3,
        WRITE_BLOCK,
        3 * WRITE_BLOCK + 5,
        0,
        11,
    ]
    .into_iter()
    .enumerate()
    {
        let chunk: Vec<u8> = (0..n).map(|k| (k + i * 31) as u8).collect();
        w.write_all(&chunk).unwrap();
        expected.extend_from_slice(&chunk);
    }
    drop(w.finish().unwrap());
    assert!(std::fs::read(&p).unwrap() == expected);

    // A handle that can't be written: the error comes back, nothing hangs.
    let mut w = ThreadedWriter::new(File::open(&p).unwrap()).unwrap();
    let big = vec![0u8; 3 * WRITE_BLOCK];
    let result = w.write_all(&big).and_then(|()| w.finish().map(drop));
    assert!(result.is_err());
}
