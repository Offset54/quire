//! Encoding detection never panics, and transcoding in segments cut at
//! clean boundaries (in parallel) decodes exactly like one continuous
//! decoder; raw offsets of the result are monotonic (ADR-0009).
//!
//! Input: [encoding selector][segment step][raw bytes...]. Built with
//! `cfg(fuzzing)`, transcoding uses 16-byte segments so small inputs have
//! many seams.

#![no_main]

use std::sync::Arc;

use encoding_rs::{
    BIG5, EUC_JP, EUC_KR, Encoding, GB18030, GBK, ISO_2022_JP, SHIFT_JIS, UTF_16BE, UTF_16LE,
    WINDOWS_1252,
};
use libfuzzer_sys::fuzz_target;
use quire_text::encoding::detect;
use quire_text::source::{MemorySource, Source};
use quire_text::transcode::{clean_boundary, decode_segment, transcode};

const ENCODINGS: [&Encoding; 11] = [
    SHIFT_JIS,
    EUC_KR,
    GBK,
    GB18030,
    BIG5,
    EUC_JP,
    UTF_16LE,
    UTF_16BE,
    WINDOWS_1252,
    ISO_2022_JP,
    encoding_rs::UTF_8,
];

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let encoding = ENCODINGS[data[0] as usize % ENCODINGS.len()];
    let step = 1 + u64::from(data[1] % 64);
    let raw = &data[2..];

    // Detection, on a whole file and on a head sample.
    let _ = detect(raw, true);
    let _ = detect(raw, false);

    let (whole, _) = decode_segment(encoding, raw);

    // Segments at clean boundaries, every `step` bytes or more.
    let src = MemorySource::new(raw.to_vec());
    let len = raw.len() as u64;
    let mut cuts = vec![0u64];
    let mut at = 0u64;
    loop {
        let next = clean_boundary(&src, encoding, at + step, len).unwrap();
        assert!(next > at || next == len, "boundary went backwards");
        if next >= len {
            break;
        }
        cuts.push(next);
        at = next;
    }
    cuts.push(len);
    let mut joined = Vec::new();
    for w in cuts.windows(2) {
        joined.extend(decode_segment(encoding, &raw[w[0] as usize..w[1] as usize]).0);
    }
    assert!(
        joined == whole,
        "segmented decode differs ({})",
        encoding.name()
    );

    // The whole pipeline (parallel waves of segments).
    let source: Arc<dyn Source> = Arc::new(MemorySource::new(raw.to_vec()));
    let t = transcode(source, encoding, 0, &std::env::temp_dir(), 2, &|_| true).unwrap();
    let utf8 = t.utf8.read_vec(0, t.utf8.len() as usize).unwrap();
    assert!(utf8 == whole, "transcode differs ({})", encoding.name());
    let text = std::str::from_utf8(&utf8).expect("transcoded text is UTF-8");
    let mut last = 0u64;
    for (i, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let r = t.raw_offset(i as u64).unwrap();
        assert!(
            r >= last && r <= len,
            "raw offset {r} after {last} (len {len})"
        );
        last = r;
    }
});
