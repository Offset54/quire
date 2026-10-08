//! Text-like fixtures: huge mixed-line TXT, single-line JSON, Markdown.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::rng::{Rng, words_to};

const MIB: u64 = 1024 * 1024;

fn writer(path: &Path) -> io::Result<BufWriter<File>> {
    Ok(BufWriter::with_capacity(8 << 20, File::create(path)?))
}

/// Cuts `s` to at most `max` bytes on a char boundary.
fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
}

/// Exactly 2 GiB. Every line starts with its 1-based number, right-aligned
/// to 10 columns and followed by "| ", so tests can check go-to-line.
pub fn mixed_lines(path: &Path, seed: u64) -> io::Result<serde_json::Value> {
    let target: u64 = 2 * 1024 * MIB;
    let mut rng = Rng::new(seed);
    let mut out = writer(path)?;
    let mut written = 0u64;
    let mut lines = 0u64;
    let mut longest = 0usize;
    let mut line = String::new();
    while written < target {
        lines += 1;
        line.clear();
        let _ = write!(line, "{lines:>10}| ");
        let body = match rng.below(1000) {
            0 => rng.range(100_000, 1 << 20),   // 0.1 %: up to 1 MiB
            1..=49 => rng.range(2_000, 20_000), // 4.9 %
            50..=149 => rng.range(200, 2_000),  // 10 %
            150..=299 => rng.range(0, 10),      // 15 %: (nearly) empty
            _ => rng.range(30, 120),            // 70 %
        } as usize;
        words_to(&mut rng, &mut line, 12 + body);
        let remaining = (target - written) as usize;
        if line.len() + 1 > remaining {
            // Last line: cut it so the file is exactly `target` bytes.
            truncate_utf8(&mut line, remaining - 1);
            while line.len() < remaining - 1 {
                line.push(' ');
            }
        }
        longest = longest.max(line.len());
        line.push('\n');
        out.write_all(line.as_bytes())?;
        written += line.len() as u64;
    }
    out.flush()?;
    Ok(serde_json::json!({ "lines": lines, "longestLineBytes": longest, "lineEnding": "LF" }))
}

fn json_string(rng: &mut Rng, out: &mut String) {
    // Words plus the characters JSON must escape.
    let mut s = String::new();
    let len = rng.range(5, 60) as usize;
    words_to(rng, &mut s, len);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    if rng.chance(100) {
        out.push_str(" \\\"quoted\\\" back\\\\slash\\ttab \\u00e9\\ud83d\\ude42");
    }
    out.push('"');
}

/// 300 MiB of minified JSON with no newline at all.
pub fn single_line_json(path: &Path, seed: u64) -> io::Result<serde_json::Value> {
    let target = (300 * MIB) as usize;
    let mut rng = Rng::new(seed);
    let mut out = writer(path)?;
    out.write_all(b"[")?;
    let mut written = 1usize;
    let mut records = 0u64;
    let mut obj = String::new();
    loop {
        obj.clear();
        if records > 0 {
            obj.push(',');
        }
        let _ = write!(
            obj,
            r#"{{"id":{records},"uuid":"{:016x}{:016x}","name":"#,
            rng.next_u64(),
            rng.next_u64()
        );
        json_string(&mut rng, &mut obj);
        let score = rng.below(10_000_000) as f64 / 1000.0 - 5000.0;
        let _ = write!(
            obj,
            r#","score":{score:.3},"active":{},"tags":["#,
            rng.chance(500)
        );
        for t in 0..rng.range(0, 5) {
            if t > 0 {
                obj.push(',');
            }
            json_string(&mut rng, &mut obj);
        }
        obj.push_str(r#"],"note":"#);
        if rng.chance(300) {
            obj.push_str("null");
        } else {
            json_string(&mut rng, &mut obj);
        }
        let _ = write!(obj, r#","nested":{{"depth":{},"values":["#, rng.range(1, 8));
        for v in 0..rng.range(1, 12) {
            if v > 0 {
                obj.push(',');
            }
            let _ = write!(obj, "{}", rng.below(1 << 31) as i64 - (1 << 30));
        }
        obj.push_str("]}}");
        if written + obj.len() + 1 > target {
            break;
        }
        out.write_all(obj.as_bytes())?;
        written += obj.len();
        records += 1;
    }
    out.write_all(b"]")?;
    out.flush()?;
    Ok(serde_json::json!({ "records": records, "lines": 1 }))
}

fn paragraph(rng: &mut Rng, i: u64, out: &mut String) {
    let mut words = String::new();
    let len = rng.range(200, 700) as usize;
    words_to(rng, &mut words, len);
    let parts: Vec<&str> = words.split(' ').collect();
    for (k, w) in parts.iter().enumerate() {
        if k > 0 {
            out.push(' ');
        }
        match rng.below(60) {
            0 => {
                let _ = write!(out, "**{w}**");
            }
            1 => {
                let _ = write!(out, "*{w}*");
            }
            2 => {
                let _ = write!(out, "`{w}`");
            }
            3 => {
                let _ = write!(out, "[{w}](https://example.com/{i}/{k})");
            }
            4 => {
                let _ = write!(out, "~~{w}~~");
            }
            5 => out.push_str("<https://example.org/auto>"),
            _ => out.push_str(w),
        }
    }
    let _ = write!(out, "[^{i}].\n\n");
}

/// 20 MiB of Markdown exercising every construct §3.1 lists.
pub fn markdown(path: &Path, seed: u64) -> io::Result<serde_json::Value> {
    let target = (20 * MIB) as usize;
    let mut rng = Rng::new(seed);
    let mut doc = String::with_capacity(target + (1 << 20));
    doc.push_str("---\ntitle: Synthetic Markdown fixture\nauthor: quire-fixtures\ntags: [benchmark, markdown]\n---\n\n");
    let mut sections = 0u64;
    while doc.len() < target {
        let i = sections;
        sections += 1;
        if i.is_multiple_of(100) {
            let _ = write!(doc, "# Part {}\n\n", i / 100 + 1);
        }
        let mut title = String::new();
        words_to(&mut rng, &mut title, 30);
        let level = if i % 7 == 6 { 3 + (i % 4) as usize } else { 2 };
        let _ = write!(doc, "{} Section {i}: {title}\n\n", "#".repeat(level));
        paragraph(&mut rng, i, &mut doc);

        doc.push_str("- first item\n  - nested item\n    - deeper item\n- [ ] open task\n- [x] done task\n\n1. one\n2. two\n   1. two-a\n3. three\n\n");

        let rows = rng.range(3, 8);
        doc.push_str("| Column A | Column B | Column C |\n|:---|:---:|---:|\n");
        for r in 0..rows {
            let mut cell = String::new();
            words_to(&mut rng, &mut cell, 12);
            let _ = writeln!(
                doc,
                "| {cell} | {r} | {:.2} |",
                rng.below(100_000) as f64 / 100.0
            );
        }
        doc.push('\n');

        let _ = write!(
            doc,
            "```rust\nfn section_{i}(x: u64) -> u64 {{\n    x.wrapping_mul({}) ^ 0x{:x}\n}}\n```\n\n",
            rng.range(3, 999),
            rng.next_u64()
        );
        doc.push_str("> A block quote with *emphasis*.\n>\n> > And a nested one.\n\n");
        let _ = write!(
            doc,
            "Inline math $a_{{{i}}} + b^2 = c$ and display math:\n\n$$\n\\sum_{{k=1}}^{{n}} k = \\frac{{n(n+1)}}{{2}}\n$$\n\n"
        );
        if i.is_multiple_of(50) {
            let _ = write!(
                doc,
                "```mermaid\ngraph TD\n  A{i}[Start] --> B{i}{{Decide}}\n  B{i} -->|yes| C{i}[Done]\n  B{i} -->|no| A{i}\n```\n\n"
            );
        }
        if i.is_multiple_of(20) {
            doc.push_str(
                "<details><summary>More</summary>\n\nHidden *details* here.\n\n</details>\n\n",
            );
        }
        let _ = write!(
            doc,
            "![Figure {i}](assets/figure-{}.png)\n\nLine with a hard break  \nnext line.\n\n",
            i % 10
        );
        let _ = write!(doc, "[^{i}]: Footnote {i}, with `code`.\n\n---\n\n");
    }
    std::fs::write(path, doc.as_bytes())?;
    Ok(serde_json::json!({ "sections": sections }))
}
