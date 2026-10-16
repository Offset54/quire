//! Chunked, parallel search equals one sequential search of the whole
//! buffer, for any pattern and document (ADR-0008, task 6). Built with
//! `cfg(fuzzing)`, chunks are 64 bytes with a 16-byte overlap so small
//! inputs cross many seams; the document is split into several pieces.
//!
//! Input: [flags][pattern length][pattern...][document...].

#![no_main]

use std::sync::OnceLock;

mod save_on_panic;

use libfuzzer_sys::fuzz_target;
use quire_core::scheduler::{CancelToken, Scheduler, SchedulerConfig};
use quire_text::buffer::TextBuffer;
use quire_text::search::{Matcher, Query, find_next, find_prev, search_all, search_range};

fn scheduler() -> &'static Scheduler {
    static S: OnceLock<Scheduler> = OnceLock::new();
    S.get_or_init(|| {
        Scheduler::new(SchedulerConfig {
            threads: 3,
            urgent_reserved: 1,
            history: 0,
            thread_name: "fuzz",
        })
    })
}

fuzz_target!(|data: &[u8]| {
    save_on_panic::guard("search", data);
    if data.len() < 3 {
        return;
    }
    let flags = data[0];
    let plen = 1 + usize::from(data[1] % 8);
    if data.len() < 2 + plen {
        return;
    }
    let query = Query {
        pattern: String::from_utf8_lossy(&data[2..2 + plen]).into_owned(),
        regex: flags & 1 != 0,
        case_sensitive: flags & 2 != 0,
        whole_word: flags & 4 != 0,
    };
    let Ok(m) = Matcher::new(&query) else {
        return;
    };
    let doc = &data[2 + plen..];
    let expected: Vec<(u64, u64)> = m
        .regex()
        .find_iter(doc)
        .filter(|x| !x.is_empty())
        .map(|x| (x.start() as u64, x.end() as u64))
        .collect();

    // The same bytes as several pieces: delete a span and put it back.
    let mut buf = TextBuffer::from_bytes(doc);
    let len = doc.len() as u64;
    if len > 2 {
        let (a, b) = (len / 3, len / 3 + len / 4);
        let span = doc[a as usize..b as usize].to_vec();
        buf.replace(a, b, b"").unwrap();
        buf.replace(a, a, &span).unwrap();
    }
    let snap = buf.snapshot();
    assert!(snap.read(0..len).unwrap() == doc);

    assert_eq!(
        search_range(&snap, &m, 0, len, usize::MAX).unwrap(),
        expected
    );
    let mut got = Vec::new();
    let p = search_all(
        &snap,
        &m,
        scheduler(),
        2,
        &CancelToken::new(),
        &mut |b, _| got.extend_from_slice(b),
    )
    .unwrap();
    assert!(p.complete);
    assert_eq!(got, expected);
    assert_eq!(find_next(&snap, &m, 0).unwrap(), expected.first().copied());
    assert_eq!(find_prev(&snap, &m, len).unwrap(), expected.last().copied());
});
