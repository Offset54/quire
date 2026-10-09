//! Random edits on the piece tree against a plain byte vector: content,
//! line counts and line lookups must agree after every edit, including
//! CR/LF pairs split and rejoined by edits (ADR-0008).
//!
//! Input: [initial length] then [op][a][b] triples; inserted bytes come
//! from a small alphabet heavy in CR, LF and multi-byte UTF-8.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quire_text::buffer::TextBuffer;

const ALPHABET: [u8; 8] = [b'a', b'\n', b'\r', b'\r', b'b', 0xE2, 0x82, 0xAC];

/// Break positions: an LF, or a CR not followed by an LF.
fn breaks(doc: &[u8]) -> Vec<usize> {
    (0..doc.len())
        .filter(|&i| doc[i] == b'\n' || (doc[i] == b'\r' && doc.get(i + 1) != Some(&b'\n')))
        .collect()
}

fn check(buf: &TextBuffer, model: &[u8]) {
    let snap = buf.snapshot();
    assert_eq!(snap.len(), model.len() as u64);
    assert!(
        snap.read(0..snap.len()).unwrap() == model,
        "content differs"
    );
    let br = breaks(model);
    assert_eq!(snap.line_count(), Some(br.len() as u64 + 1), "line count");
    for (k, &p) in br.iter().enumerate().take(64) {
        let start = (p + 1) as u64;
        assert_eq!(
            snap.line_start(k as u64 + 1),
            Some(Some(start)),
            "line {}",
            k + 1
        );
        assert_eq!(snap.line_of(start), Some(k as u64 + 1), "line of {start}");
        assert_eq!(snap.line_start_containing(start).unwrap(), start);
    }
    assert_eq!(snap.line_start(br.len() as u64 + 1), Some(None));
}

fuzz_target!(|data: &[u8]| {
    let Some((&n0, rest)) = data.split_first() else {
        return;
    };
    let n0 = (n0 % 48) as usize;
    if rest.len() < n0 {
        return;
    }
    let mut model: Vec<u8> = rest[..n0]
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()])
        .collect();
    let mut buf = TextBuffer::from_bytes(&model);
    check(&buf, &model);
    let mut ops = rest[n0..].chunks_exact(3);
    for op in ops.by_ref().take(200) {
        let (kind, a, b) = (op[0], u64::from(op[1]), u64::from(op[2]));
        let len = model.len() as u64;
        let from = a % (len + 1);
        let to = (from + b % 9).min(len);
        let text: Vec<u8> = (0..(kind >> 2) % 6)
            .map(|i| {
                ALPHABET[(usize::from(kind) + usize::from(i) * 3 + b as usize) % ALPHABET.len()]
            })
            .collect();
        let (to, text) = match kind % 3 {
            0 => (from, text),     // insert
            1 => (to, Vec::new()), // delete
            _ => (to, text),       // replace
        };
        buf.replace(from, to, &text).unwrap();
        model.splice(from as usize..to as usize, text);
        check(&buf, &model);
    }
});
