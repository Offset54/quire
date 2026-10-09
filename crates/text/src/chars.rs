//! Character-level helpers over raw document bytes, which may contain
//! invalid UTF-8: every invalid byte is one unit of its own (shown as
//! U+FFFD), so offsets stay byte-exact.

use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthChar;

/// `(offset, byte length, char)` for each character of `bytes`; an invalid
/// byte is `(offset, 1, None)`.
pub fn chars(bytes: &[u8]) -> impl Iterator<Item = (usize, usize, Option<char>)> + '_ {
    // Each valid run is validated once and then walked (linear overall).
    let mut at = 0usize;
    let mut run = "".char_indices();
    let mut run_base = 0usize;
    std::iter::from_fn(move || {
        if let Some((i, c)) = run.next() {
            return Some((run_base + i, c.len_utf8(), Some(c)));
        }
        if at >= bytes.len() {
            return None;
        }
        let valid = match std::str::from_utf8(&bytes[at..]) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&bytes[at..at + e.valid_up_to()]).expect("validated"),
        };
        if valid.is_empty() {
            at += 1;
            return Some((at - 1, 1, None));
        }
        run_base = at;
        at += valid.len();
        run = valid.char_indices();
        let (i, c) = run.next().expect("non-empty");
        Some((run_base + i, c.len_utf8(), Some(c)))
    })
}

/// Display width in columns of `c` at column `col` (tabs advance to the
/// next multiple of `tab`). Control characters and invalid bytes are shown
/// as one-column symbols.
pub fn width(c: Option<char>, col: u32, tab: u32) -> u32 {
    match c {
        None => 1,
        Some('\t') => tab.max(1) - col % tab.max(1),
        Some(c) if (c as u32) < 0x20 || c == '\u{7f}' => 1,
        Some(c) => c.width().unwrap_or(1) as u32,
    }
}

/// Whether `c` is a CJK-style character that lines may wrap before or after.
pub fn is_wide(c: char) -> bool {
    c.width() == Some(2)
}

/// Text for display, and the UTF-16 indices of characters that stand for a
/// single raw byte (invalid bytes as U+FFFD, C0 controls as U+2400 symbols).
/// Tabs are kept.
pub fn display_text(bytes: &[u8]) -> (String, Vec<u32>) {
    let mut s = String::with_capacity(bytes.len());
    let mut subst = Vec::new();
    let mut u16_len = 0u32;
    for (_, _, c) in chars(bytes) {
        let shown = match c {
            None => {
                subst.push(u16_len);
                '\u{FFFD}'
            }
            Some('\t') => '\t',
            Some(c) if (c as u32) < 0x20 => {
                subst.push(u16_len);
                char::from_u32(0x2400 + c as u32).expect("control picture")
            }
            Some('\u{7f}') => {
                subst.push(u16_len);
                '\u{2421}'
            }
            Some(c) => c,
        };
        s.push(shown);
        u16_len += shown.len_utf16() as u32;
    }
    (s, subst)
}

/// Byte offset of UTF-16 index `u16_index` in `bytes` as shown by
/// [`display_text`]. Indices inside a surrogate pair round down.
pub fn u16_to_byte(bytes: &[u8], u16_index: u32) -> usize {
    let mut u = 0u32;
    for (off, _, c) in chars(bytes) {
        let w = match c {
            None => 1,
            Some(c) if (c as u32) < 0x20 || c == '\u{7f}' => 1,
            Some(c) => c.len_utf16() as u32,
        };
        if u + w > u16_index {
            return off;
        }
        u += w;
    }
    bytes.len()
}

/// UTF-16 index of byte offset `byte` in `bytes` as shown by [`display_text`].
pub fn byte_to_u16(bytes: &[u8], byte: usize) -> u32 {
    let mut u = 0u32;
    for (off, _, c) in chars(bytes) {
        if off >= byte {
            break;
        }
        u += match c {
            None => 1,
            Some(c) if (c as u32) < 0x20 || c == '\u{7f}' => 1,
            Some(c) => c.len_utf16() as u32,
        };
    }
    u
}

/// The grapheme boundary after `at` in `bytes` (which starts at a char
/// boundary). Invalid bytes are single units; CRLF is one.
pub fn next_grapheme(bytes: &[u8], at: usize) -> usize {
    if at >= bytes.len() {
        return bytes.len();
    }
    let valid_len = match std::str::from_utf8(&bytes[at..]) {
        Ok(s) => s.len(),
        Err(e) => e.valid_up_to(),
    };
    if valid_len == 0 {
        return at + 1;
    }
    let s = std::str::from_utf8(&bytes[at..at + valid_len]).expect("validated");
    let mut cursor = GraphemeCursor::new(0, s.len(), true);
    match cursor.next_boundary(s, 0) {
        Ok(Some(b)) => at + b,
        _ => at + s.chars().next().map_or(1, char::len_utf8),
    }
}

/// The grapheme boundary before `at` in `bytes`.
pub fn prev_grapheme(bytes: &[u8], at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    // Where the valid UTF-8 run that ends at `at` begins.
    let mut run_start = 0;
    for (off, n, c) in chars(&bytes[..at]) {
        if c.is_none() {
            run_start = off + n;
        }
    }
    if run_start == at {
        return at - 1; // an invalid byte just before
    }
    let s = std::str::from_utf8(&bytes[run_start..at]).expect("valid run");
    let mut cursor = GraphemeCursor::new(s.len(), s.len(), true);
    match cursor.prev_boundary(s, 0) {
        Ok(Some(b)) => run_start + b,
        _ => run_start + s.char_indices().last().map_or(0, |(i, _)| i),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Space,
    Word,
    Punct,
    Break,
    Wide,
}

pub fn class(c: Option<char>) -> Class {
    match c {
        None => Class::Punct,
        Some('\n' | '\r') => Class::Break,
        Some(c) if c.is_whitespace() => Class::Space,
        Some(c) if is_wide(c) && !c.is_ascii() => Class::Wide,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_bytes_are_single_units() {
        let b = b"a\xFFb\xE2\x82c";
        let v: Vec<_> = chars(b).collect();
        assert_eq!(
            v,
            vec![
                (0, 1, Some('a')),
                (1, 1, None),
                (2, 1, Some('b')),
                (3, 1, None),
                (4, 1, None),
                (5, 1, Some('c'))
            ]
        );
        let (s, subst) = display_text(b"x\x01\xFF\ty");
        assert_eq!(s, "x\u{2401}\u{FFFD}\ty");
        assert_eq!(subst, vec![1, 2]);
    }

    #[test]
    fn utf16_mapping_round_trips() {
        let b = "a😀한\u{0301}\x02b".as_bytes();
        for (off, _, _) in chars(b) {
            let u = byte_to_u16(b, off);
            assert_eq!(u16_to_byte(b, u), off);
        }
        assert_eq!(byte_to_u16(b, b.len()), 7);
        assert_eq!(u16_to_byte(b, 2), 1); // inside the surrogate pair
    }

    #[test]
    fn graphemes() {
        let b = "e\u{0301}x\r\ny👍🏽z".as_bytes();
        assert_eq!(next_grapheme(b, 0), 3);
        assert_eq!(next_grapheme(b, 4), 6);
        assert_eq!(prev_grapheme(b, 6), 4);
        assert_eq!(prev_grapheme(b, 3), 0);
        let end = b.len() - 1;
        assert_eq!(prev_grapheme(b, end), 7);
        assert_eq!(next_grapheme(b"\xFFa", 0), 1);
        assert_eq!(prev_grapheme(b"a\xFF", 2), 1);
    }

    #[test]
    fn widths() {
        assert_eq!(width(Some('\t'), 3, 4), 1);
        assert_eq!(width(Some('\t'), 4, 4), 4);
        assert_eq!(width(Some('한'), 0, 4), 2);
        assert_eq!(width(Some('\u{0301}'), 0, 4), 0);
        assert_eq!(width(None, 0, 4), 1);
    }
}
