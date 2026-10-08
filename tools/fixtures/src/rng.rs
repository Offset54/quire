//! SplitMix64: tiny, fast, and identical on every platform, which is all a
//! deterministic fixture generator needs.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0). The tiny modulo bias is irrelevant here.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `per_mille / 1000`.
    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// Words for synthetic prose, mostly English with some Korean, accented
/// Latin and an emoji, so text paths see multi-byte UTF-8.
pub const WORDS: &[&str] = &[
    "the",
    "of",
    "and",
    "to",
    "in",
    "is",
    "that",
    "for",
    "it",
    "as",
    "with",
    "was",
    "on",
    "be",
    "at",
    "by",
    "this",
    "from",
    "or",
    "an",
    "which",
    "are",
    "document",
    "render",
    "layout",
    "glyph",
    "paragraph",
    "table",
    "image",
    "section",
    "page",
    "slide",
    "viewer",
    "editor",
    "buffer",
    "index",
    "search",
    "result",
    "stream",
    "measure",
    "budget",
    "latency",
    "frame",
    "texture",
    "anchor",
    "column",
    "margin",
    "ligature",
    "kerning",
    "baseline",
    "cursor",
    "selection",
    "revision",
    "fixture",
    "benchmark",
    "quickly",
    "carefully",
    "although",
    "however",
    "therefore",
    "meanwhile",
    "résumé",
    "naïve",
    "façade",
    "Straße",
    "文書",
    "한국어",
    "문서",
    "편집기",
    "성능",
    "테스트",
    "데이터",
    "🙂",
];

/// Appends words until `out` reaches at least `target` bytes.
pub fn words_to(rng: &mut Rng, out: &mut String, target: usize) {
    let mut first = true;
    while out.len() < target {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(rng.pick(WORDS));
    }
}
