//! Word count for the status bar (§7), streamed over the text in chunks.
//!
//! A word is a run of non-whitespace characters with at least one letter or
//! digit (so Markdown markers like `#`, `-` or `**` alone don't count). Han
//! ideographs, kana and other scripts written without spaces count one word
//! per character, as word processors count them; Hangul is written with
//! spaces and counts by runs.

/// Counts words across chunks (chunks may split UTF-8 sequences).
#[derive(Debug, Default)]
pub struct WordCounter {
    words: u64,
    in_word: bool,
    counted: bool,
    carry: Vec<u8>,
}

/// Scripts counted per character.
fn per_char(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff        // Hiragana, Katakana
        | 0x3400..=0x4dbf      // CJK Extension A
        | 0x4e00..=0x9fff      // CJK Unified Ideographs
        | 0xf900..=0xfaff      // CJK Compatibility Ideographs
        | 0x20000..=0x2fa1f    // CJK Extensions B–F and supplements
        | 0x0e00..=0x0e7f      // Thai
        | 0x0e80..=0x0eff      // Lao
        | 0x1000..=0x109f      // Myanmar
        | 0x1780..=0x17ff      // Khmer
    )
}

impl WordCounter {
    pub fn new() -> Self {
        Self::default()
    }

    fn char(&mut self, c: char) {
        if c.is_whitespace() {
            self.in_word = false;
            self.counted = false;
            return;
        }
        if per_char(c) {
            self.words += 1;
            // A word run before or after it is a separate word.
            self.in_word = false;
            self.counted = false;
            return;
        }
        if !self.in_word {
            self.in_word = true;
            self.counted = false;
        }
        if !self.counted && c.is_alphanumeric() {
            self.words += 1;
            self.counted = true;
        }
    }

    /// Feeds the next chunk.
    pub fn push(&mut self, bytes: &[u8]) {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let mut rest: &[u8] = &buf;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    for c in s.chars() {
                        self.char(c);
                    }
                    break;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    let s = std::str::from_utf8(&rest[..good]).unwrap_or("");
                    for c in s.chars() {
                        self.char(c);
                    }
                    match e.error_len() {
                        // An invalid sequence: a separator.
                        Some(n) => {
                            self.char(' ');
                            rest = &rest[good + n..];
                        }
                        // Cut at the chunk's end: keep it for the next one.
                        None => {
                            self.carry = rest[good..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn finish(mut self) -> u64 {
        if !self.carry.is_empty() {
            self.char(' ');
        }
        self.words
    }
}

/// Words in `bytes`.
pub fn count(bytes: &[u8]) -> u64 {
    let mut w = WordCounter::new();
    w.push(bytes);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_not_markers() {
        assert_eq!(count(b"# Title\n\n- one **two** three\n---\n"), 4);
        assert_eq!(count("대한민국은 민주공화국이다.".as_bytes()), 2);
        assert_eq!(count("日本語のテキスト".as_bytes()), 8);
        assert_eq!(count("mixed中文words".as_bytes()), 4);
        assert_eq!(count(b"a\xffb"), 2);
        assert_eq!(count(b""), 0);
    }

    #[test]
    fn chunks_split_anywhere() {
        let text = "Hello wörld, 한국어 텍스트 and 日本 [link](url) `code`\n".repeat(20);
        let whole = count(text.as_bytes());
        for size in [1, 2, 3, 5, 7, 64] {
            let mut w = WordCounter::new();
            for c in text.as_bytes().chunks(size) {
                w.push(c);
            }
            assert_eq!(w.finish(), whole, "chunk size {size}");
        }
    }
}
