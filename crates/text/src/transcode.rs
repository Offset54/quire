//! Transcoding non-UTF-8 files to UTF-8 (ADR-0009).
//!
//! The piece tree always holds UTF-8. A UTF-8 file is used in place; any
//! other encoding is decoded once into a UTF-8 buffer (in memory when small,
//! otherwise a delete-on-close temp file that is then mapped).
//!
//! The raw file is cut into **segments** at clean boundaries, where a fresh
//! decoder produces exactly what a continuous decode would:
//! - single-byte encodings: anywhere;
//! - UTF-16: an even offset not just after a high surrogate;
//! - multi-byte legacy encodings (Shift_JIS, CP949, GB18030, Big5, EUC-JP):
//!   just after a byte below 0x30, which is always a whole character in
//!   these encodings, never part of a multi-byte sequence;
//! - stateful encodings (ISO-2022-JP): no cuts.
//!
//! Segments decode in parallel, and their (raw offset, UTF-8 offset) table is
//! kept. On save, an untouched range of the original is mapped back to its
//! raw bytes, which are copied as they are: re-encoding isn't always
//! byte-exact (duplicate mappings in Shift_JIS, malformed sequences).

use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use encoding_rs::{CoderResult, Encoding, UTF_16BE, UTF_16LE};

use crate::encoding::{is_multibyte_legacy, is_stateful};
use crate::source::{MappedSource, MemorySource, Source};

/// Target segment size, in raw bytes.
pub const SEGMENT: u64 = 64 * 1024;
/// Transcoded output up to this size stays in memory.
pub const IN_MEMORY_MAX: u64 = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub raw: u64,
    pub utf8: u64,
}

/// A decoded original.
#[derive(Clone, Debug)]
pub struct Transcoded {
    pub encoding: &'static Encoding,
    /// The raw file, BOM included.
    pub raw: Arc<dyn Source>,
    pub bom_len: u64,
    /// The UTF-8 text (BOM excluded).
    pub utf8: Arc<dyn Source>,
    /// Segment starts, sorted; the first is (bom_len, 0).
    pub segments: Vec<Segment>,
    /// Whether any malformed sequence was decoded as U+FFFD.
    pub had_errors: bool,
}

/// Finds the next clean boundary at or after `at` (≤ `end`).
pub fn clean_boundary(
    raw: &dyn Source,
    encoding: &'static Encoding,
    at: u64,
    end: u64,
) -> io::Result<u64> {
    if at >= end || encoding.is_single_byte() {
        return Ok(at.min(end));
    }
    if encoding == UTF_16LE || encoding == UTF_16BE {
        let mut at = at + (at % 2);
        if at >= end {
            return Ok(end);
        }
        if at >= 2 {
            let mut u = [0u8; 2];
            raw.read_at(at - 2, &mut u)?;
            let unit = if encoding == UTF_16LE {
                u16::from_le_bytes(u)
            } else {
                u16::from_be_bytes(u)
            };
            if (0xD800..0xDC00).contains(&unit) {
                at += 2;
            }
        }
        return Ok(at.min(end));
    }
    if is_multibyte_legacy(encoding) {
        // Just after the first byte < 0x30 at or after `at - 1`.
        let mut pos = at.saturating_sub(1);
        let mut buf = vec![0u8; 4096];
        while pos < end {
            let n = (buf.len() as u64).min(end - pos) as usize;
            raw.read_at(pos, &mut buf[..n])?;
            if let Some(i) = buf[..n].iter().position(|&b| b < 0x30) {
                return Ok((pos + i as u64 + 1).min(end));
            }
            pos += n as u64;
        }
        return Ok(end);
    }
    // Stateful or unknown: never cut.
    Ok(end)
}

fn segment_starts(raw: &dyn Source, encoding: &'static Encoding, bom: u64) -> io::Result<Vec<u64>> {
    let end = raw.len();
    let mut starts = vec![bom];
    if is_stateful(encoding) {
        return Ok(starts);
    }
    let mut at = bom;
    loop {
        let next = clean_boundary(raw, encoding, at + SEGMENT, end)?;
        if next >= end {
            break;
        }
        starts.push(next);
        at = next;
    }
    Ok(starts)
}

/// Decodes one raw segment. Returns the UTF-8 and whether it had errors.
pub fn decode_segment(encoding: &'static Encoding, raw: &[u8]) -> (Vec<u8>, bool) {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let cap = decoder
        .max_utf8_buffer_length(raw.len())
        .unwrap_or(raw.len() * 3 + 16);
    let mut out = vec![0u8; cap];
    let (result, read, written, had_errors) = decoder.decode_to_utf8(raw, &mut out, true);
    debug_assert_eq!(result, CoderResult::InputEmpty);
    debug_assert_eq!(read, raw.len());
    out.truncate(written);
    (out, had_errors)
}

/// Decodes `raw` (whose first `bom_len` bytes are a BOM) to UTF-8, in
/// parallel segments on `threads` threads. Output larger than
/// [`IN_MEMORY_MAX`] goes to a delete-on-close temp file in `temp_dir`.
/// `progress(raw_bytes_done)` is called as segments complete; returning
/// false cancels (an `Interrupted` error).
pub fn transcode(
    raw: Arc<dyn Source>,
    encoding: &'static Encoding,
    bom_len: u64,
    temp_dir: &Path,
    threads: usize,
    progress: &dyn Fn(u64) -> bool,
) -> io::Result<Transcoded> {
    let starts = segment_starts(&*raw, encoding, bom_len)?;
    let end = raw.len();
    let ranges: Vec<Range<u64>> = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| s..starts.get(i + 1).copied().unwrap_or(end))
        .collect();

    enum Out {
        Memory(Vec<u8>),
        File(std::io::BufWriter<std::fs::File>, u64),
    }
    let mut out = if end - bom_len <= IN_MEMORY_MAX {
        Out::Memory(Vec::with_capacity((end - bom_len) as usize))
    } else {
        std::fs::create_dir_all(temp_dir)?;
        Out::File(
            std::io::BufWriter::with_capacity(4 << 20, tempfile::tempfile_in(temp_dir)?),
            0,
        )
    };
    let mut segments = Vec::with_capacity(ranges.len());
    let mut utf8_len = 0u64;
    let mut had_errors = false;
    let threads = threads.max(1);
    // Waves of `threads × 8` segments: decode in parallel, write in order.
    for wave in ranges.chunks(threads * 8) {
        let results: Vec<io::Result<(Vec<u8>, bool)>> = std::thread::scope(|s| {
            let per = wave.len().div_ceil(threads);
            let handles: Vec<_> = wave
                .chunks(per)
                .map(|group| {
                    let raw = &raw;
                    s.spawn(move || {
                        group
                            .iter()
                            .map(|r| {
                                let bytes = raw.read_vec(r.start, (r.end - r.start) as usize)?;
                                Ok(decode_segment(encoding, &bytes))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("decode thread panicked"))
                .collect()
        });
        for (r, res) in wave.iter().zip(results) {
            let (bytes, errors) = res?;
            had_errors |= errors;
            segments.push(Segment {
                raw: r.start,
                utf8: utf8_len,
            });
            utf8_len += bytes.len() as u64;
            match &mut out {
                Out::Memory(v) => v.extend_from_slice(&bytes),
                Out::File(w, n) => {
                    w.write_all(&bytes)?;
                    *n += bytes.len() as u64;
                }
            }
        }
        let done = wave.last().map_or(end, |r| r.end);
        if !progress(done) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "transcoding cancelled",
            ));
        }
    }
    let utf8: Arc<dyn Source> = match out {
        Out::Memory(v) => Arc::new(MemorySource::new(v)),
        Out::File(w, n) => {
            let file = w.into_inner().map_err(io::IntoInnerError::into_error)?;
            if n == 0 {
                Arc::new(MemorySource::new(Vec::new()))
            } else {
                Arc::new(MappedSource::new(file)?)
            }
        }
    };
    Ok(Transcoded {
        encoding,
        raw,
        bom_len,
        utf8,
        segments,
        had_errors,
    })
}

impl Transcoded {
    /// Whether untouched ranges can be copied raw on save. Not for stateful
    /// encodings: a range's bytes depend on escape sequences before it.
    pub fn supports_raw_copy(&self) -> bool {
        !is_stateful(self.encoding)
    }

    /// Raw offset of UTF-8 offset `x` (a character boundary of the decoded
    /// text). Re-decodes at most one segment.
    pub fn raw_offset(&self, x: u64) -> io::Result<u64> {
        if x >= self.utf8.len() {
            return Ok(self.raw.len());
        }
        let i = match self.segments.binary_search_by(|s| s.utf8.cmp(&x)) {
            Ok(i) => return Ok(self.segments[i].raw),
            Err(i) => i - 1,
        };
        let seg = self.segments[i];
        let raw_end = self.segments.get(i + 1).map_or(self.raw.len(), |s| s.raw);
        let bytes = self.raw.read_vec(seg.raw, (raw_end - seg.raw) as usize)?;
        Ok(seg.raw + raw_split(self.encoding, &bytes, x - seg.utf8) as u64)
    }

    /// The raw byte range that decodes to UTF-8 range `r`.
    pub fn raw_range(&self, r: Range<u64>) -> io::Result<Range<u64>> {
        Ok(self.raw_offset(r.start)?..self.raw_offset(r.end)?)
    }
}

/// Raw bytes of `raw` that decode to the first `target` bytes of its UTF-8.
/// Feeds the decoder one byte at a time; where one input byte releases more
/// than one character (a malformed lead byte followed by ASCII), a target
/// between them maps to before that byte.
pub fn raw_split(encoding: &'static Encoding, raw: &[u8], target: u64) -> usize {
    if target == 0 {
        return 0;
    }
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let mut out = [0u8; 64];
    let mut written = 0u64;
    for i in 0..raw.len() {
        let last = i + 1 == raw.len();
        let (_, _, w, _) = decoder.decode_to_utf8(&raw[i..=i], &mut out, last);
        let before = written;
        written += w as u64;
        if written == target {
            return i + 1;
        }
        if written > target {
            debug_assert!(before < target);
            return i;
        }
    }
    raw.len()
}

#[cfg(test)]
mod tests;
