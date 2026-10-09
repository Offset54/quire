use encoding_rs::{BIG5, EUC_KR, GB18030, ISO_2022_JP, SHIFT_JIS, WINDOWS_1251};

use super::*;
use crate::encoding::is_stateful;
use crate::encoding::tests::{JA, KO, RU, ZH_HANS, ZH_HANT, encode};

fn sample(text: &str, target: usize) -> String {
    let mut s = String::new();
    let mut i = 0;
    while s.len() < target {
        s.push_str(&format!("{i:>6}: "));
        s.push_str(text);
        i += 1;
    }
    s
}

fn utf16(s: &str, le: bool) -> Vec<u8> {
    s.encode_utf16()
        .flat_map(|u| if le { u.to_le_bytes() } else { u.to_be_bytes() })
        .collect()
}

fn check(encoding: &'static Encoding, raw: Vec<u8>, bom: u64) {
    let dir = tempfile::tempdir().unwrap();
    let whole = encoding
        .decode_without_bom_handling(&raw[bom as usize..])
        .0
        .into_owned();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(raw.clone()));
    let t = transcode(Arc::clone(&src), encoding, bom, dir.path(), 4, &|_| true).unwrap();
    let utf8 = t.utf8.read_vec(0, t.utf8.len() as usize).unwrap();
    assert_eq!(
        String::from_utf8(utf8).unwrap(),
        whole,
        "{}",
        encoding.name()
    );
    assert!(t.segments.len() > 1 || ISO_2022_JP == encoding || raw.len() < 70_000);
    // Every segment start decodes cleanly: re-check the table.
    for s in &t.segments {
        assert_eq!(t.raw_offset(s.utf8).unwrap(), s.raw);
    }
    assert_eq!(t.supports_raw_copy(), !is_stateful(encoding));
    if !t.supports_raw_copy() {
        return;
    }
    // Raw ranges of char-aligned UTF-8 ranges decode back to the same text.
    let chars: Vec<usize> = whole.char_indices().map(|(i, _)| i).collect();
    for k in 0..40 {
        let a = chars[(k * 7919) % chars.len()];
        let b = chars[((k * 7919) % chars.len() + 1 + k * 131).min(chars.len() - 1)].max(a);
        let r = t.raw_range(a as u64..b as u64).unwrap();
        let back = encoding.decode_without_bom_handling(&raw[r.start as usize..r.end as usize]);
        assert_eq!(&*back.0, &whole[a..b], "{} {a}..{b}", encoding.name());
    }
    let all = t.raw_range(0..t.utf8.len()).unwrap();
    assert_eq!(all, bom..raw.len() as u64);
}

#[test]
fn legacy_encodings_transcode_in_segments_and_map_back() {
    for (e, text) in [
        (EUC_KR, KO),
        (SHIFT_JIS, JA),
        (GB18030, ZH_HANS),
        (BIG5, ZH_HANT),
        (WINDOWS_1251, RU),
        (ISO_2022_JP, JA),
    ] {
        check(e, encode(e, &sample(text, 300_000)), 0);
    }
}

#[test]
fn utf16_with_surrogates_and_bom() {
    let text = sample("emoji 😀 at odd places 🎉, and 한글\n", 200_000);
    let mut le = vec![0xFF, 0xFE];
    le.extend(utf16(&text, true));
    check(UTF_16LE, le, 2);
    check(UTF_16BE, utf16(&text, false), 0);
}

#[test]
fn malformed_input_decodes_the_same_in_segments() {
    let mut raw = encode(SHIFT_JIS, &sample(JA, 200_000));
    // Lone lead bytes before ASCII, and a truncated sequence at the end.
    for i in (1000..raw.len()).step_by(9973) {
        raw[i] = 0x82;
        raw[i + 1] = b'A';
    }
    raw.push(0x82);
    let dir = tempfile::tempdir().unwrap();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(raw.clone()));
    let t = transcode(src, SHIFT_JIS, 0, dir.path(), 3, &|_| true).unwrap();
    assert!(t.had_errors);
    let whole = SHIFT_JIS.decode_without_bom_handling(&raw).0.into_owned();
    let utf8 = t.utf8.read_vec(0, t.utf8.len() as usize).unwrap();
    assert_eq!(String::from_utf8(utf8).unwrap(), whole);
}

#[test]
fn large_output_goes_to_a_mapped_temp_file() {
    let raw = encode(
        WINDOWS_1251,
        &sample(RU, (IN_MEMORY_MAX + 1000) as usize / 2 + 10),
    );
    // Cyrillic doubles in UTF-8, so the output passes the in-memory cap.
    let dir = tempfile::tempdir().unwrap();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(raw.clone()));
    let t = transcode(src, WINDOWS_1251, 0, dir.path(), 4, &|_| true).unwrap();
    assert!(
        raw.len() as u64 <= IN_MEMORY_MAX || t.utf8.kind() == crate::source::SourceKind::Mapped
    );
    let probe = t.utf8.read_vec(t.utf8.len() - 10, 10).unwrap();
    let whole_tail = WINDOWS_1251
        .decode_without_bom_handling(&raw[raw.len() - 5..])
        .0;
    assert_eq!(
        &probe[probe.len() - whole_tail.len()..],
        whole_tail.as_bytes()
    );
}

#[test]
fn cancel_stops_transcoding() {
    let raw = encode(EUC_KR, &sample(KO, 2_000_000));
    let dir = tempfile::tempdir().unwrap();
    let src: Arc<dyn Source> = Arc::new(MemorySource::new(raw));
    let err = transcode(src, EUC_KR, 0, dir.path(), 2, &|done| done < 500_000).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Interrupted);
}
#[test]
fn utf16_cut_never_splits_a_pair_after_a_run_of_high_surrogates() {
    // D800 D800 DC00: one decoder gives U+FFFD then U+10000. A cut after
    // the first D800 that skips only one unit would land between the pair.
    let units: [u16; 6] = [0x41, 0xD800, 0xD800, 0xD800, 0xDC00, 0x42];
    for le in [true, false] {
        let raw: Vec<u8> = units
            .iter()
            .flat_map(|u| if le { u.to_le_bytes() } else { u.to_be_bytes() })
            .collect();
        let encoding = if le { UTF_16LE } else { UTF_16BE };
        let src = MemorySource::new(raw.clone());
        let whole = decode_segment(encoding, &raw).0;
        for at in 0..=raw.len() as u64 {
            let cut = clean_boundary(&src, encoding, at, raw.len() as u64).unwrap();
            let (a, b) = raw.split_at(cut as usize);
            let mut joined = decode_segment(encoding, a).0;
            joined.extend(decode_segment(encoding, b).0);
            assert_eq!(joined, whole, "cut at {cut} (asked {at}), le {le}");
        }
    }
}
