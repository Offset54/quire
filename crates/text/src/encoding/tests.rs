use encoding_rs::{WINDOWS_1251, WINDOWS_1253};

use super::*;

pub(crate) const KO: &str = "대한민국은 민주공화국이다. 대한민국의 주권은 국민에게 있고, 모든 권력은 국민으로부터 나온다. 모든 국민은 인간으로서의 존엄과 가치를 가지며, 행복을 추구할 권리를 가진다.\n";
pub(crate) const JA: &str = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。\n";
pub(crate) const ZH_HANS: &str = "中华人民共和国是工人阶级领导的、以工农联盟为基础的人民民主专政的社会主义国家。社会主义制度是中华人民共和国的根本制度。\n";
pub(crate) const ZH_HANT: &str = "臺灣是一個位於東亞的島嶼，面積約三萬六千平方公里，人口約兩千三百萬人。臺北市是政治、經濟與文化的中心。\n";
pub(crate) const RU: &str = "Все люди рождаются свободными и равными в своем достоинстве и правах. Они наделены разумом и совестью.\n";
pub(crate) const EL: &str = "Όλοι οι άνθρωποι γεννιούνται ελεύθεροι και ίσοι στην αξιοπρέπεια και τα δικαιώματα. Είναι προικισμένοι με λογική.\n";

pub(crate) fn encode(e: &'static Encoding, s: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = e.encode(s);
    assert!(!unmappable, "{} can't encode the sample", e.name());
    bytes.into_owned()
}

#[test]
fn boms_win() {
    let d = detect(b"\xEF\xBB\xBFhello", true);
    assert_eq!((d.encoding, d.bom_len, d.by), (UTF_8, 3, DetectedBy::Bom));
    let d = detect(b"\xFF\xFEh\0i\0", true);
    assert_eq!((d.encoding, d.bom_len), (UTF_16LE, 2));
    assert!(!d.binary);
    let d = detect(b"\xFE\xFF\0h\0i", true);
    assert_eq!((d.encoding, d.bom_len), (UTF_16BE, 2));
}

#[test]
fn utf16_without_bom_is_recognized() {
    let le: Vec<u8> = "plain text, no byte order mark\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(detect(&le, true).encoding, UTF_16LE);
    let be: Vec<u8> = "plain text, no byte order mark\n"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    assert_eq!(detect(&be, true).encoding, UTF_16BE);
}

#[test]
fn declarations_are_honored() {
    let d = detect(br#"<?xml version="1.0" encoding="Shift_JIS"?><a/>"#, true);
    assert_eq!((d.encoding, d.by), (SHIFT_JIS, DetectedBy::Declared));
    let d = detect(br#"<html><head><META charset="euc-kr"></head>"#, true);
    assert_eq!(d.encoding, EUC_KR);
    let d = detect(
        br#"<meta http-equiv="Content-Type" content="text/html; charset=gbk">"#,
        true,
    );
    assert_eq!(d.encoding, GB18030);
    let d = detect(
        b"#!/usr/bin/env python\n# -*- coding: big5 -*-\nprint(1)\n",
        true,
    );
    assert_eq!(d.encoding, BIG5);
    // UTF-16 declared in ASCII bytes is ignored.
    let d = detect(br#"<?xml version="1.0" encoding="UTF-16"?><a/>"#, true);
    assert_eq!((d.encoding, d.by), (UTF_8, DetectedBy::Default));
}

#[test]
fn ascii_and_utf8_are_utf8() {
    assert_eq!(detect(b"hello\nworld\n", true).by, DetectedBy::Default);
    assert_eq!(detect(b"", true).encoding, UTF_8);
    let d = detect(KO.as_bytes(), true);
    assert_eq!((d.encoding, d.by), (UTF_8, DetectedBy::Heuristic));
    // A multi-byte character cut by the end of the sample is still UTF-8.
    let cut = &KO.as_bytes()[..KO.len() - 3];
    assert_eq!(detect(cut, false).encoding, UTF_8);
}

#[test]
fn legacy_encodings_are_guessed() {
    let cases: [(&'static Encoding, &str); 6] = [
        (EUC_KR, KO),
        (SHIFT_JIS, JA),
        (GB18030, ZH_HANS),
        (BIG5, ZH_HANT),
        (WINDOWS_1251, RU),
        (WINDOWS_1253, EL),
    ];
    for (e, text) in cases {
        let bytes = encode(e, &text.repeat(4));
        let d = detect(&bytes, true);
        assert_eq!(d.encoding, e, "expected {} for its sample", e.name());
        assert!(!d.binary);
    }
}

#[test]
fn binary_is_flagged() {
    let mut b = b"MZ\x90\0\x03\0\0\0".to_vec();
    b.extend(std::iter::repeat_n(0xAB, 200));
    assert!(detect(&b, true).binary);
    assert!(!detect(b"tab\tseparated\tvalues\r\n", true).binary);
}
