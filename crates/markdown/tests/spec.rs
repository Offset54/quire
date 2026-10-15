//! CommonMark and GFM spec tests through the block pipeline (M2 task 3):
//! every example is indexed, analyzed and rendered one block at a time
//! (`render_document`), before sanitization, and compared with the expected
//! HTML after normalization (`support/normalize.rs`).
//!
//! The suites are fetched by `tools/corpora/fetch-markdown-spec.ps1` into
//! `.cache/corpora/markdown` (CC-BY-SA, not committed). Without them the test
//! is skipped, unless `QUIRE_SPEC_REQUIRED` is set (CI sets it).
//!
//! Known failures are listed in `spec-exceptions.txt` with their reasons
//! (and in `docs/markdown-spec-exceptions.md`). The test fails on any other
//! failure, on a listed exception that now passes, and below 99 %.

#[path = "support/normalize.rs"]
mod normalize;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use quire_markdown::document::render_document;
use quire_markdown::options::Flavor;
use quire_markdown::render::RenderOptions;

const FENCE: &str = "````````````````````````````````";

struct Example {
    number: usize,
    section: String,
    disabled: bool,
    extensions: Vec<String>,
    markdown: String,
    html: String,
}

fn examples(text: &str) -> Vec<Example> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
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
        let mut md = String::new();
        let mut html = String::new();
        let mut in_html = false;
        for l in lines.by_ref() {
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
            section: section.clone(),
            disabled: rest.contains("disabled"),
            extensions: rest.split_whitespace().skip(1).map(String::from).collect(),
            markdown: md,
            html,
        });
    }
    out
}

fn corpora() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.cache/corpora/markdown")
}

/// `(suite, number)` → reason.
fn exceptions() -> Vec<(String, usize, String)> {
    let text = include_str!("spec-exceptions.txt");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut parts = l.splitn(3, ' ');
            let suite = parts.next().unwrap().to_owned();
            let number = parts.next().unwrap().parse().expect("example number");
            let reason = parts.next().unwrap_or("").to_owned();
            (suite, number, reason)
        })
        .collect()
}

const ALL_GFM: &[&str] = &[
    "table",
    "strikethrough",
    "autolink",
    "tagfilter",
    "tasklist",
    "footnotes",
];

#[test]
fn spec_suites() {
    let dir = corpora();
    let suites = [
        ("commonmark", "commonmark-0.31.2-spec.txt"),
        ("gfm", "gfm-spec.txt"),
        ("gfm-ext", "gfm-extensions.txt"),
    ];
    if !dir.join(suites[0].1).exists() {
        assert!(
            std::env::var_os("QUIRE_SPEC_REQUIRED").is_none(),
            "spec corpora missing: run tools/corpora/fetch-markdown-spec.ps1"
        );
        eprintln!("spec corpora not fetched; skipped (tools/corpora/fetch-markdown-spec.ps1)");
        return;
    }
    let expected_failures: BTreeSet<(String, usize)> =
        exceptions().into_iter().map(|(s, n, _)| (s, n)).collect();
    let mut failures: BTreeSet<(String, usize)> = BTreeSet::new();
    let (mut total, mut passed) = (0usize, 0usize);
    let mut report = String::new();
    for (suite, file) in suites {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        let (mut n, mut ok) = (0, 0);
        for ex in examples(&text) {
            if ex.disabled {
                continue;
            }
            // Extensions as cmark-gfm's runner enables them.
            let flavor = match suite {
                "commonmark" => Flavor::COMMONMARK,
                "gfm" => Flavor::gfm(&ex.extensions.iter().map(String::as_str).collect::<Vec<_>>()),
                _ => Flavor::gfm(ALL_GFM),
            };
            let opts = RenderOptions {
                sourcepos: false,
                heading_ids: false,
                flavor,
                document: true,
                sanitize: None,
                highlight: false,
            };
            let got = std::panic::catch_unwind(|| render_document(ex.markdown.as_bytes(), &opts))
                .unwrap_or_else(|_| "<<panic>>".into());
            let pass = ex.html.trim() == "<IGNORE>"
                || normalize::normalize(&got) == normalize::normalize(&ex.html);
            n += 1;
            if pass {
                ok += 1;
            } else {
                failures.insert((suite.to_owned(), ex.number));
                if !expected_failures.contains(&(suite.to_owned(), ex.number)) {
                    eprintln!(
                        "UNEXPECTED FAILURE {suite} {} ({})\n--- markdown\n{}--- expected\n{}--- got\n{}",
                        ex.number, ex.section, ex.markdown, ex.html, got
                    );
                }
            }
        }
        report.push_str(&format!(
            "{suite}: {ok}/{n} ({:.2} %)\n",
            ok as f64 * 100.0 / n as f64
        ));
        total += n;
        passed += ok;
    }
    let rate = passed as f64 * 100.0 / total as f64;
    report.push_str(&format!("all: {passed}/{total} ({rate:.2} %)\n"));
    eprint!("{report}");
    if let Some(out) = std::env::var_os("QUIRE_SPEC_REPORT") {
        std::fs::write(out, &report).unwrap();
    }
    let unexpected: Vec<_> = failures.difference(&expected_failures).collect();
    let fixed: Vec<_> = expected_failures.difference(&failures).collect();
    assert!(unexpected.is_empty(), "unexpected failures: {unexpected:?}");
    assert!(
        fixed.is_empty(),
        "listed exceptions now pass, remove them: {fixed:?}"
    );
    assert!(rate >= 99.0, "pass rate {rate:.2} % is under 99 %");
}

/// The product configuration (every extension on) over the CommonMark suite:
/// informational, it shows what the extensions change in plain CommonMark.
#[test]
#[ignore = "informational; run with --ignored"]
fn commonmark_with_every_extension() {
    let path = corpora().join("commonmark-0.31.2-spec.txt");
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let opts = RenderOptions {
        sourcepos: false,
        heading_ids: false,
        flavor: Flavor::PRODUCT,
        document: true,
        sanitize: None,
        highlight: false,
    };
    let (mut n, mut ok) = (0, 0);
    for ex in examples(&text) {
        n += 1;
        let got = render_document(ex.markdown.as_bytes(), &opts);
        if normalize::normalize(&got) == normalize::normalize(&ex.html) {
            ok += 1;
        } else {
            eprintln!(
                "differs with every extension: {} ({})",
                ex.number, ex.section
            );
        }
    }
    eprintln!("commonmark with every extension: {ok}/{n}");
}
