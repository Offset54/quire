//! Save cost against a raw sequential write (§4 "save at sequential write
//! speed"): `cargo run -p quire-text --release --example savebench -- <copy>`.
//! `<copy>` is overwritten by the save; use a scratch copy of a fixture.

use std::io::Write;
use std::time::Instant;

use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::document::Action;
use quire_text::file::{OnUnmappable, OpenSettings, SaveOptions, commit_save, open, prepare_save};

fn seqwrite(path: &std::path::Path, bytes: u64) -> f64 {
    let buf: Vec<u8> = (0..8usize << 20).map(|i| b'a' + (i % 26) as u8).collect();
    let t = Instant::now();
    let mut f = std::fs::File::create(path).unwrap();
    let mut left = bytes;
    while left > 0 {
        let n = (buf.len() as u64).min(left) as usize;
        f.write_all(&buf[..n]).unwrap();
        left -= n as u64;
    }
    f.sync_all().unwrap();
    let s = t.elapsed().as_secs_f64();
    drop(f);
    std::fs::remove_file(path).unwrap();
    s
}

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("file"));
    let g = MemoryGovernor::new(GovernorConfig::default());
    let tmp = std::env::temp_dir();
    let mut f = open(
        &path,
        &g,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: tmp.clone(),
            threads: 4,
            progress: &|_| true,
        },
    )
    .unwrap();
    let e = f.original_index();
    e.source.prefetch(0, e.source.len());
    e.index
        .scan_range(&*e.source, 0, e.index.chunk_count(), None)
        .unwrap();
    f.doc.buffer_mut().refresh_index();
    let mid = f.doc.len() / 2;
    f.doc
        .apply(&Action::SetSelections {
            ranges: vec![(mid, mid)],
        })
        .unwrap();
    f.doc
        .apply(&Action::Type {
            text: "SAVE-BENCH ".into(),
        })
        .unwrap();
    let t = Instant::now();
    let prepared = prepare_save(
        &f.doc.snapshot(),
        f.origin.as_ref(),
        f.doc.format,
        f.path.as_deref(),
        None,
        OnUnmappable::Fail,
    )
    .unwrap();
    let prepare = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let report = commit_save(
        &mut f,
        prepared,
        &SaveOptions {
            on_unmappable: OnUnmappable::Fail,
            threads: 4,
            temp_dir: &tmp,
        },
    )
    .unwrap();
    let commit = t.elapsed().as_secs_f64();
    let raw = seqwrite(&path.with_extension("seqwrite"), report.bytes_written);
    println!(
        "{{\"prepareS\":{prepare:.3},\"commitS\":{commit:.3},\"saveS\":{:.3},\"seqwriteS\":{raw:.3},\"ratio\":{:.3},\"bytes\":{}}}",
        prepare + commit,
        (prepare + commit) / raw,
        report.bytes_written
    );
}
