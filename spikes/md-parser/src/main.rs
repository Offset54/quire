//! M2 task 1 spike (ADR-0026): comrak vs. pulldown-cmark.
//!
//! `md-parser spec <spec.txt> <suite> <out.json>`: renders every spec example
//! with both parsers (the comparison with the expected HTML is done by
//! `compare.py`, which uses the spec's own `normalize.py`).
//!
//! `md-parser bench <file.md> <out.json>`: throughput on the 20 MiB fixture,
//! parse only and parse + render, plus the cost of parsing one top-level block
//! in isolation and whether the concatenation of isolated blocks matches the
//! whole-document render.
//!
//! `md-parser features <out.json>`: how each parser handles front matter,
//! footnotes, `$` math, extended autolinks and source positions.

mod normalize;

use std::fmt::Write as _;
use std::time::Instant;

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("spec") => spec(&args[2], &args[3], &args[4]),
        Some("bench") => bench(&args[2], &args[3]),
        Some("features") => features(&args[2]),
        _ => {
            eprintln!("usage: md-parser spec|bench|features ...");
            std::process::exit(2);
        }
    }
}

const FENCE: &str = "````````````````````````````````";

struct Example {
    number: usize,
    line: usize,
    section: String,
    disabled: bool,
    extensions: Vec<String>,
    markdown: String,
    html: String,
}

fn examples(text: &str) -> Vec<Example> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        if let Some(h) = line.strip_prefix('#') {
            section = h.trim_start_matches('#').trim().to_string();
            continue;
        }
        let Some(rest) = line.strip_prefix(FENCE) else {
            continue;
        };
        let rest = rest.trim();
        if !rest.starts_with("example") {
            continue;
        }
        let disabled = rest.contains("disabled");
        let extensions = rest.split_whitespace().skip(1).map(String::from).collect();
        let mut md = String::new();
        let mut html = String::new();
        let mut in_html = false;
        for (_, l) in lines.by_ref() {
            if l.starts_with(FENCE) {
                break;
            }
            if !in_html && l == "." {
                in_html = true;
                continue;
            }
            let target = if in_html { &mut html } else { &mut md };
            target.push_str(&l.replace('\u{2192}', "\t"));
            target.push('\n');
        }
        out.push(Example {
            number: out.len() + 1,
            line: n + 1,
            section: section.clone(),
            disabled,
            extensions,
            markdown: md,
            html,
        });
    }
    out
}

const ALL_GFM: &[&str] = &[
    "table",
    "strikethrough",
    "autolink",
    "tagfilter",
    "tasklist",
    "footnotes",
];

/// The extensions an example runs with, as cmark-gfm's spec runner does:
/// none for CommonMark, the ones named in the example header for the GFM
/// spec, all of them for extensions.txt.
fn suite_extensions<'a>(suite: &str, header: &'a [String]) -> Vec<&'a str> {
    match suite {
        "commonmark" => Vec::new(),
        "gfm" => header.iter().map(String::as_str).collect(),
        _ => ALL_GFM.to_vec(),
    }
}

fn comrak_options(ext: &[&str]) -> comrak::Options<'static> {
    let mut o = comrak::Options::default();
    o.render.r#unsafe = true;
    o.extension.table = ext.contains(&"table");
    o.extension.strikethrough = ext.contains(&"strikethrough");
    o.extension.autolink = ext.contains(&"autolink");
    o.extension.tasklist = ext.contains(&"tasklist");
    o.extension.footnotes = ext.contains(&"footnotes");
    // comrak 0.56 removed `tagfilter`.
    o
}

fn pulldown_options(ext: &[&str]) -> pulldown_cmark::Options {
    use pulldown_cmark::Options as O;
    let mut o = O::empty();
    o.set(O::ENABLE_TABLES, ext.contains(&"table"));
    o.set(O::ENABLE_STRIKETHROUGH, ext.contains(&"strikethrough"));
    o.set(O::ENABLE_TASKLISTS, ext.contains(&"tasklist"));
    o.set(O::ENABLE_FOOTNOTES, ext.contains(&"footnotes"));
    // pulldown-cmark has no extended autolinks and no tag filter.
    o
}

fn comrak_html(md: &str, o: &comrak::Options) -> String {
    comrak::markdown_to_html(md, o)
}

fn pulldown_html(md: &str, o: pulldown_cmark::Options) -> String {
    let mut s = String::new();
    pulldown_cmark::html::push_html(&mut s, pulldown_cmark::Parser::new_ext(md, o));
    s
}

fn spec(path: &str, suite: &str, out: &str) {
    let text = std::fs::read_to_string(path).expect("read spec");
    let mut rows = Vec::new();
    let (mut total, mut c_pass, mut p_pass) = (0, 0, 0);
    let mut c_fail = Vec::new();
    let mut p_fail = Vec::new();
    for ex in examples(&text) {
        let ext = suite_extensions(suite, &ex.extensions);
        let co = comrak_options(&ext);
        let po = pulldown_options(&ext);
        // A parser that panics fails the example; it must not stop the run.
        let c = std::panic::catch_unwind(|| comrak_html(&ex.markdown, &co))
            .unwrap_or_else(|_| "<<panic>>".into());
        let p = std::panic::catch_unwind(|| pulldown_html(&ex.markdown, po))
            .unwrap_or_else(|_| "<<panic>>".into());
        // `<IGNORE>`: only checks that the parser doesn't crash (cmark-gfm).
        let ignore = ex.html.trim() == "<IGNORE>";
        let want = normalize::normalize(&ex.html);
        let c_ok = ignore || normalize::normalize(&c) == want;
        let p_ok = ignore || normalize::normalize(&p) == want;
        if !ex.disabled {
            total += 1;
            if c_ok {
                c_pass += 1;
            } else {
                c_fail.push(json!([ex.number, ex.section]));
            }
            if p_ok {
                p_pass += 1;
            } else {
                p_fail.push(json!([ex.number, ex.section]));
            }
        }
        rows.push(json!({
            "number": ex.number, "line": ex.line, "section": ex.section,
            "disabled": ex.disabled, "markdown": ex.markdown, "expected": ex.html,
            "comrak": c, "comrak_ok": c_ok, "pulldown": p, "pulldown_ok": p_ok,
        }));
    }
    let summary = json!({
        "suite": suite, "spec": path, "examples": total,
        "comrak": { "pass": c_pass, "rate": c_pass as f64 / total as f64, "failed": c_fail },
        "pulldown": { "pass": p_pass, "rate": p_pass as f64 / total as f64, "failed": p_fail },
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    std::fs::write(out, serde_json::to_vec_pretty(&rows).unwrap()).expect("write");
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut ms = Vec::new();
    let mut last = None;
    for _ in 0..runs {
        let t = Instant::now();
        let r = f();
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        last = Some(r);
    }
    (median(ms), last.unwrap())
}

/// Product options: everything §3.1 lists that each parser supports.
fn comrak_product() -> comrak::Options<'static> {
    let mut o = comrak_options(ALL_GFM);
    o.extension.front_matter_delimiter = Some("---".into());
    o.extension.math_dollars = true;
    o.render.sourcepos = true;
    o
}

fn pulldown_product() -> pulldown_cmark::Options {
    pulldown_options(ALL_GFM)
        | pulldown_cmark::Options::ENABLE_GFM
        | pulldown_cmark::Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | pulldown_cmark::Options::ENABLE_MATH
}

/// Top-level block byte ranges from pulldown-cmark's offset iterator.
fn pulldown_blocks(md: &str, o: pulldown_cmark::Options) -> Vec<std::ops::Range<usize>> {
    use pulldown_cmark::Event;
    let mut depth = 0usize;
    let mut out = Vec::new();
    for (ev, range) in pulldown_cmark::Parser::new_ext(md, o).into_offset_iter() {
        match ev {
            Event::Start(_) => {
                if depth == 0 {
                    out.push(range);
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ if depth == 0 => out.push(range),
            _ => {}
        }
    }
    out
}

/// Top-level block line ranges from comrak's source positions.
fn comrak_blocks(md: &str, o: &comrak::Options) -> Vec<(usize, usize)> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, o);
    root.children()
        .map(|n| {
            let sp = n.data().sourcepos;
            (sp.start.line, sp.end.line)
        })
        .collect()
}

fn bench(path: &str, out: &str) {
    let md = std::fs::read_to_string(path).expect("read fixture");
    let mb = md.len() as f64 / 1e6;
    let runs = 5;
    let co = comrak_product();
    let po = pulldown_product();
    let mut r = serde_json::Map::new();
    r.insert("bytes".into(), json!(md.len()));

    let (ms, nodes) = time(runs, || {
        let arena = comrak::Arena::new();
        let root = comrak::parse_document(&arena, &md, &co);
        root.descendants().count()
    });
    r.insert("comrak_parse_ms".into(), json!(ms));
    r.insert("comrak_parse_mb_s".into(), json!(mb / ms * 1e3));
    r.insert("comrak_nodes".into(), json!(nodes));
    let (ms, len) = time(runs, || comrak::markdown_to_html(&md, &co).len());
    r.insert("comrak_render_ms".into(), json!(ms));
    r.insert("comrak_render_mb_s".into(), json!(mb / ms * 1e3));
    r.insert("comrak_html_bytes".into(), json!(len));

    let (ms, events) = time(runs, || pulldown_cmark::Parser::new_ext(&md, po).count());
    r.insert("pulldown_parse_ms".into(), json!(ms));
    r.insert("pulldown_parse_mb_s".into(), json!(mb / ms * 1e3));
    r.insert("pulldown_events".into(), json!(events));
    let (ms, _) = time(runs, || {
        pulldown_cmark::Parser::new_ext(&md, po)
            .into_offset_iter()
            .count()
    });
    r.insert("pulldown_offset_parse_ms".into(), json!(ms));
    let (ms, len) = time(runs, || pulldown_html(&md, po).len());
    r.insert("pulldown_render_ms".into(), json!(ms));
    r.insert("pulldown_render_mb_s".into(), json!(mb / ms * 1e3));
    r.insert("pulldown_html_bytes".into(), json!(len));

    // Block structure: how many top-level blocks, and the cost of finding them.
    let (ms, blocks) = time(3, || pulldown_blocks(&md, po));
    r.insert("pulldown_blocks".into(), json!(blocks.len()));
    r.insert("pulldown_blocks_ms".into(), json!(ms));
    let (ms, cblocks) = time(3, || comrak_blocks(&md, &co));
    r.insert("comrak_blocks".into(), json!(cblocks.len()));
    r.insert("comrak_blocks_ms".into(), json!(ms));

    // Isolated block parsing: parse each top-level block (a window of 2,000
    // blocks from the middle) on its own and time it per block.
    let mid = blocks.len() / 2;
    let window = &blocks[mid..(mid + 2000).min(blocks.len())];
    let mut per_c = Vec::new();
    let mut per_p = Vec::new();
    let mut biggest = 0usize;
    for b in window {
        let s = &md[b.clone()];
        biggest = biggest.max(s.len());
        let t = Instant::now();
        let _ = comrak::markdown_to_html(s, &co);
        per_c.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now();
        let _ = pulldown_html(s, po);
        per_p.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let p95 = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() * 95 / 100]
    };
    r.insert(
        "isolated_block_us_comrak_p50".into(),
        json!(median(per_c.clone())),
    );
    r.insert("isolated_block_us_comrak_p95".into(), json!(p95(per_c)));
    r.insert(
        "isolated_block_us_pulldown_p50".into(),
        json!(median(per_p.clone())),
    );
    r.insert("isolated_block_us_pulldown_p95".into(), json!(p95(per_p)));
    r.insert("isolated_block_biggest_bytes".into(), json!(biggest));

    // Does rendering block by block (with the gap between blocks) give the
    // whole-document HTML? Differences are expected only where a block depends
    // on another (reference definitions, footnotes, tight/loose lists).
    let head = &md[..md.len().min(2_000_000)];
    let hb = pulldown_blocks(head, po);
    let whole = pulldown_html(head, po);
    let mut pieces = String::new();
    for b in &hb {
        pieces.push_str(&pulldown_html(&head[b.clone()], po));
    }
    r.insert(
        "pulldown_isolated_equals_whole_2mb".into(),
        json!(pieces == whole),
    );
    r.insert(
        "pulldown_isolated_len_2mb".into(),
        json!([pieces.len(), whole.len()]),
    );

    let v = Value::Object(r);
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    std::fs::write(out, serde_json::to_vec_pretty(&v).unwrap()).expect("write");
}

const SAMPLE: &str = "---\ntitle: T\ntags: [a, b]\n---\n\n# Heading *one*\n\nText with $a^2$ math, www.example.com and https://example.org/x?y=1 autolinks, ~~gone~~, and a note[^n].\n\n$$\n\\sum_k k\n$$\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n- [ ] open\n- [x] done\n  > quoted\n\n```rust\nfn f() {}\n```\n\n<details><summary>S</summary>\n\n*inside*\n\n</details>\n\n[ref]: https://example.com/ref\n\nUses [ref] far from its definition.\n\n[^n]: The note.\n";

fn features(out: &str) {
    let co = comrak_product();
    let po = pulldown_product();
    let mut r = serde_json::Map::new();
    r.insert("sample".into(), json!(SAMPLE));
    r.insert(
        "comrak_html".into(),
        json!(comrak::markdown_to_html(SAMPLE, &co)),
    );
    r.insert("pulldown_html".into(), json!(pulldown_html(SAMPLE, po)));

    // Source positions: comrak gives line:column spans per node (block and
    // inline); pulldown-cmark gives byte ranges per event.
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, SAMPLE, &co);
    let mut cpos = String::new();
    for n in root.descendants() {
        let d = n.data();
        let depth = n.ancestors().count() - 1;
        let _ = writeln!(
            cpos,
            "{}{:?} {}:{}-{}:{}",
            "  ".repeat(depth),
            std::mem::discriminant(&d.value),
            d.sourcepos.start.line,
            d.sourcepos.start.column,
            d.sourcepos.end.line,
            d.sourcepos.end.column
        );
        let _ = cpos.pop();
        let _ = writeln!(cpos, " {}", node_name(&d.value));
    }
    r.insert("comrak_sourcepos".into(), json!(cpos));
    let mut ppos = String::new();
    for (ev, range) in pulldown_cmark::Parser::new_ext(SAMPLE, po).into_offset_iter() {
        let _ = writeln!(ppos, "{range:?} {ev:?}");
    }
    r.insert("pulldown_offsets".into(), json!(ppos));
    std::fs::write(out, serde_json::to_vec_pretty(&Value::Object(r)).unwrap()).expect("write");
}

fn node_name(v: &comrak::nodes::NodeValue) -> String {
    let s = format!("{v:?}");
    s.split(['(', ' ', '{']).next().unwrap_or("").to_string()
}
