//! Highlighting cost per language (ADR-0029): each sample repeated to about
//! 200 lines (the largest block the preview highlights whole) and to the
//! 256 KiB cap; median of 15 runs after a warm-up.
//!
//! `cargo run --release -p quire-highlight --example hlbench`

use std::time::Instant;

use quire_highlight::{LANGUAGES, MAX_BYTES, highlight};

fn median_ms(code: &str, lang: &str) -> f64 {
    let _ = highlight(lang, code);
    let mut t: Vec<f64> = (0..15)
        .map(|_| {
            let s = Instant::now();
            let _ = highlight(lang, code);
            s.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t[7]
}

fn main() {
    println!("language      200 lines (ms)   KiB    cap (ms)   MB/s   first use (ms)");
    for l in LANGUAGES.iter().filter(|l| !l.hidden) {
        let name = l.names[0];
        let lines = l.sample.lines().count().max(1);
        let first = {
            let s = Instant::now();
            let _ = highlight(name, l.sample);
            s.elapsed().as_secs_f64() * 1000.0
        };
        let small = l.sample.repeat(200usize.div_ceil(lines));
        let mut big = String::new();
        while big.len() + l.sample.len() <= MAX_BYTES {
            big.push_str(l.sample);
        }
        let a = median_ms(&small, name);
        let b = median_ms(&big, name);
        println!(
            "{name:<12} {a:>10.2} {:>12.0} {b:>10.1} {:>6.1} {first:>12.1}",
            small.len() as f64 / 1024.0,
            big.len() as f64 / 1e6 / (b / 1000.0)
        );
    }
}
