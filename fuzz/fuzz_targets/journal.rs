//! A recovery journal is hostile input (it may be torn, corrupted or
//! crafted): parsing never panics, every record it accepts re-encodes to
//! itself, and replaying the records into a document either applies them
//! or returns an error, never panics.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quire_text::document::TextDocument;
use quire_text::journal::{decode_record, encode_record, parse};

fuzz_target!(|data: &[u8]| {
    // The bytes as one record payload.
    if let Some((r, t)) = decode_record(data) {
        assert_eq!(decode_record(&encode_record(&r, t)), Some((r, t)));
    }
    let Some(j) = parse(data) else {
        return;
    };
    assert!(j.ignored_tail as usize <= data.len());
    let mut doc = TextDocument::from_bytes(b"line one\r\nline two\nthree");
    for (r, t) in &j.records {
        assert_eq!(
            decode_record(&encode_record(r, *t)).as_ref(),
            Some(&(r.clone(), *t))
        );
        if doc.apply_record(r).is_err() {
            break;
        }
    }
});
