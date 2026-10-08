//! Encoding detection (§6.1; ADR-0009). Order: BOM → declared encoding →
//! heuristic (UTF-16 without BOM, UTF-8 validity, then chardetng). The user
//! can always override and re-decode.

use encoding_rs::{
    BIG5, EUC_KR, Encoding, GB18030, GBK, ISO_2022_JP, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE,
};
use serde::Serialize;

/// How the encoding was chosen, shown in the status bar's tooltip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectedBy {
    Bom,
    Declared,
    Heuristic,
    /// Empty or pure ASCII: UTF-8 (a superset).
    Default,
    /// Chosen by the user.
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detection {
    pub encoding: &'static Encoding,
    /// Length of the byte-order mark at the start of the file (0, 2 or 3).
    pub bom_len: usize,
    pub by: DetectedBy,
    /// NULs or control characters suggest this isn't text (§6.1).
    pub binary: bool,
}

/// Bytes of the file used for detection.
pub const SAMPLE: usize = 64 * 1024;

/// Encodings offered in the UI, by WHATWG label. encoding_rs names CP949
/// "EUC-KR" (the WHATWG EUC-KR decoder is Windows-949).
pub const OFFERED: &[&str] = &[
    "UTF-8",
    "UTF-16LE",
    "UTF-16BE",
    "EUC-KR",
    "Shift_JIS",
    "gb18030",
    "Big5",
    "windows-1250",
    "windows-1251",
    "windows-1252",
    "windows-1253",
    "windows-1254",
    "windows-1255",
    "windows-1256",
    "windows-1257",
    "windows-1258",
];

/// Display name: CP949 for encoding_rs's EUC-KR, which is what it decodes.
pub fn display_name(e: &'static Encoding) -> &'static str {
    if e == EUC_KR {
        "CP949 (EUC-KR)"
    } else {
        e.name()
    }
}

/// Detects the encoding of a file from its first bytes. `is_whole` says
/// whether `head` is the entire file.
pub fn detect(head: &[u8], is_whole: bool) -> Detection {
    let sample = &head[..head.len().min(SAMPLE)];
    if let Some((encoding, bom_len)) = Encoding::for_bom(sample) {
        return Detection {
            encoding,
            bom_len,
            by: DetectedBy::Bom,
            binary: binary_after_decode(sample, encoding),
        };
    }
    if let Some(encoding) = utf16_without_bom(sample) {
        return Detection {
            encoding,
            bom_len: 0,
            by: DetectedBy::Heuristic,
            binary: false,
        };
    }
    let binary = looks_binary(sample);
    if let Some(encoding) = declared(sample) {
        return Detection {
            encoding,
            bom_len: 0,
            by: DetectedBy::Declared,
            binary,
        };
    }
    if sample.is_ascii() {
        return Detection {
            encoding: UTF_8,
            bom_len: 0,
            by: DetectedBy::Default,
            binary,
        };
    }
    if valid_utf8_prefix(sample, is_whole) {
        return Detection {
            encoding: UTF_8,
            bom_len: 0,
            by: DetectedBy::Heuristic,
            binary,
        };
    }
    let mut d = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    d.feed(sample, is_whole);
    let guess = d.guess(None, chardetng::Utf8Detection::Allow);
    Detection {
        encoding: normalize(guess),
        bom_len: 0,
        by: DetectedBy::Heuristic,
        binary,
    }
}

/// GBK is decoded as its superset GB18030.
fn normalize(e: &'static Encoding) -> &'static Encoding {
    if e == GBK { GB18030 } else { e }
}

/// Valid UTF-8, allowing a sequence cut off by the end of the sample.
fn valid_utf8_prefix(sample: &[u8], is_whole: bool) -> bool {
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(e) => !is_whole && e.error_len().is_none() && sample.len() - e.valid_up_to() < 4,
    }
}

/// UTF-16 text without a BOM is mostly ASCII with a zero byte in every
/// other position.
fn utf16_without_bom(sample: &[u8]) -> Option<&'static Encoding> {
    let n = sample.len() / 2;
    if n < 8 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for pair in sample.as_chunks::<2>().0 {
        even += usize::from(pair[0] == 0);
        odd += usize::from(pair[1] == 0);
    }
    if odd * 10 >= n * 4 && even * 20 <= n {
        Some(UTF_16LE)
    } else if even * 10 >= n * 4 && odd * 20 <= n {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// NUL bytes, or many C0 controls other than tab, CR, LF, FF and ESC.
fn looks_binary(sample: &[u8]) -> bool {
    if memchr::memchr(0, sample).is_some() {
        return true;
    }
    let controls = sample
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B))
        .count();
    controls * 100 > sample.len().max(1) * 5
}

fn binary_after_decode(sample: &[u8], e: &'static Encoding) -> bool {
    if e == UTF_16LE || e == UTF_16BE {
        let (text, _) = e.decode_without_bom_handling(&sample[2.min(sample.len())..]);
        return text.contains('\0');
    }
    looks_binary(sample)
}

/// An encoding declared in the content: an XML declaration, an HTML meta
/// tag, or an editor coding comment (Emacs, Vim, PEP 263) in the first two
/// lines. UTF-16 declarations in ASCII-compatible bytes are ignored, as the
/// HTML standard does.
fn declared(sample: &[u8]) -> Option<&'static Encoding> {
    let head = &sample[..sample.len().min(4096)];
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let found = xml_declaration(&lower)
        .or_else(|| meta_charset(&lower))
        .or_else(|| coding_comment(&lower))?;
    let e = Encoding::for_label(found)?;
    (e != UTF_16LE && e != UTF_16BE).then(|| normalize(e))
}

fn label_at(s: &[u8], at: usize) -> Option<&[u8]> {
    let s = &s[at..];
    let start = s
        .iter()
        .position(|&b| !matches!(b, b' ' | b'"' | b'\'' | b'='))?;
    let s = &s[start..];
    let end = s
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')))
        .unwrap_or(s.len());
    (end > 0).then(|| &s[..end])
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

fn xml_declaration(lower: &[u8]) -> Option<&[u8]> {
    if !lower.starts_with(b"<?xml") {
        return None;
    }
    let end = find(lower, b"?>")?;
    let decl = &lower[..end];
    let at = find(decl, b"encoding")? + b"encoding".len();
    label_at(decl, at)
}

fn meta_charset(lower: &[u8]) -> Option<&[u8]> {
    let mut from = 0;
    while let Some(m) = find(&lower[from..], b"<meta") {
        let tag_start = from + m;
        let tag_end = tag_start + find(&lower[tag_start..], b">")?;
        let tag = &lower[tag_start..tag_end];
        if let Some(at) = find(tag, b"charset") {
            return label_at(tag, at + b"charset".len());
        }
        from = tag_end;
    }
    None
}

fn coding_comment(lower: &[u8]) -> Option<&[u8]> {
    // First two lines only (PEP 263, Emacs).
    let mut end = lower.len();
    if let Some(a) = memchr::memchr(b'\n', lower) {
        end = memchr::memchr(b'\n', &lower[a + 1..]).map_or(lower.len(), |b| a + 1 + b);
    }
    let lines = &lower[..end];
    for key in [&b"coding:"[..], b"coding=", b"fileencoding="] {
        if let Some(at) = find(lines, key) {
            return label_at(lines, at + key.len());
        }
    }
    None
}

/// Whether a decoder for `e` can start at any byte < 0x30 boundary
/// (non-stateful), so transcoding can run in parallel segments.
pub fn is_stateful(e: &'static Encoding) -> bool {
    e == ISO_2022_JP
}

/// Encodings where every byte is one character.
pub fn is_single_byte(e: &'static Encoding) -> bool {
    e.is_single_byte()
}

/// Multi-byte legacy encodings, for which segment boundaries are moved past
/// a byte below 0x30 (never part of a multi-byte sequence in any of them).
pub fn is_multibyte_legacy(e: &'static Encoding) -> bool {
    [SHIFT_JIS, EUC_KR, GB18030, GBK, BIG5, encoding_rs::EUC_JP].contains(&e)
}

#[cfg(test)]
pub(crate) mod tests;
