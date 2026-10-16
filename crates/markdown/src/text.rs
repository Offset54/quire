//! Byte access to the document, and UTF-8 that keeps byte offsets.

use std::borrow::Cow;
use std::io;
use std::ops::Range;

use quire_text::buffer::Snapshot;

/// The document's bytes. Implemented by the text engine's [`Snapshot`] (never
/// one whole-file buffer, §5.5) and by byte slices for tests.
pub trait Text {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads `range`, clamped to the text.
    fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>>;
}

impl Text for Snapshot {
    fn len(&self) -> u64 {
        Snapshot::len(self)
    }

    fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        Snapshot::read(self, range)
    }
}

impl Text for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        let end = (range.end.min(Text::len(self))) as usize;
        let start = (range.start as usize).min(end);
        Ok(self[start..end].to_vec())
    }
}

impl Text for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }

    fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        Text::read(self.as_slice(), range)
    }
}

/// Byte that stands in for each byte of an invalid UTF-8 sequence while
/// parsing, so offsets don't move (U+001A SUBSTITUTE; plain text to CommonMark).
pub const SUBSTITUTE: u8 = 0x1a;

/// The bytes as UTF-8 of the same length: each byte of an invalid sequence
/// becomes [`SUBSTITUTE`]. Borrowed when the bytes are valid.
pub fn same_len_utf8(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => {
            let mut out = Vec::with_capacity(bytes.len());
            let mut rest = bytes;
            loop {
                match std::str::from_utf8(rest) {
                    Ok(s) => {
                        out.extend_from_slice(s.as_bytes());
                        break;
                    }
                    Err(e) => {
                        let good = e.valid_up_to();
                        out.extend_from_slice(&rest[..good]);
                        let bad = e.error_len().unwrap_or(rest.len() - good);
                        out.extend(std::iter::repeat_n(SUBSTITUTE, bad));
                        rest = &rest[good + bad..];
                    }
                }
            }
            // Every invalid byte was replaced by an ASCII byte.
            Cow::Owned(String::from_utf8(out).expect("valid by construction"))
        }
    }
}

/// comrak skips a byte order mark at the start of whatever it parses, but
/// past the document's start U+FEFF is text (a zero-width no-break space):
/// a stretch or block that starts on such a line would parse differently
/// alone than in the whole document (`\u{feff}> q` is a paragraph, not a
/// quote). For text parsed from `start > 0`, a leading U+FEFF becomes
/// U+2060 WORD JOINER: the same length, also zero-width and no-break, and
/// not skipped. (Found by the `md_scan` fuzz target.)
pub fn mid_document(bytes: &[u8], start: u64) -> Cow<'_, [u8]> {
    const BOM: &[u8] = "\u{feff}".as_bytes();
    const WORD_JOINER: &[u8] = "\u{2060}".as_bytes();
    match bytes.strip_prefix(BOM) {
        Some(rest) if start > 0 => Cow::Owned([WORD_JOINER, rest].concat()),
        _ => Cow::Borrowed(bytes),
    }
}

/// Byte offsets of line starts in `s` (always starts with 0). A line ends
/// after `\n`, or after a `\r` not followed by `\n` (CommonMark line endings).
pub fn line_starts(s: &[u8]) -> Vec<usize> {
    let mut out = vec![0];
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'\n' => out.push(i + 1),
            b'\r' if s.get(i + 1) != Some(&b'\n') => out.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_len_keeps_offsets() {
        let b = b"a\xffb\xe2\x82c\xe2\x82\xac";
        let s = same_len_utf8(b);
        assert_eq!(s.len(), b.len());
        assert_eq!(s.as_bytes(), b"a\x1ab\x1a\x1ac\xe2\x82\xac");
        assert!(matches!(same_len_utf8(b"plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn line_starts_follow_commonmark_endings() {
        assert_eq!(line_starts(b"a\nb\r\nc\rd"), vec![0, 2, 5, 7]);
        assert_eq!(line_starts(b""), vec![0]);
        assert_eq!(line_starts(b"x\n"), vec![0, 2]);
    }
}
