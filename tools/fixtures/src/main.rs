//! Deterministic fixture generators for the §4 benchmark scenarios
//! (MASTER_PROMPT §10; M0 task 11).
//!
//! ```text
//! cargo run -p quire-fixtures --release -- [--out DIR] [NAME ...]
//! ```
//!
//! With no names, every fixture is generated. Output goes to `.cache/fixtures`
//! (ignored by git) along with `manifest.json`: size, SHA-256 and seed of each
//! file. The same generator version always produces byte-identical files.
//! Validation that needs external tools (qpdf, Word, PowerPoint) lives in
//! `tools/fixtures/validate.ps1`.

mod images;
mod office;
mod pdf;
mod rng;
mod text;

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

/// Bump when any generator's output changes, so results can name the
/// fixture version they were measured on.
const GENERATOR_VERSION: u32 = 1;

struct Fixture {
    name: &'static str,
    file: &'static str,
    seed: u64,
    description: &'static str,
    generate: fn(&Path, u64) -> io::Result<serde_json::Value>,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "txt-2g",
        file: "mixed-lines-2gib.txt",
        seed: 0x7478_7432,
        description: "2 GiB UTF-8 text, LF, mixed line lengths (0 B to 1 MiB); each line starts with its 1-based number",
        generate: text::mixed_lines,
    },
    Fixture {
        name: "json-300m",
        file: "single-line-300mib.json",
        seed: 0x6a73_6f6e,
        description: "300 MiB minified JSON on a single line",
        generate: text::single_line_json,
    },
    Fixture {
        name: "md-20m",
        file: "document-20mib.md",
        seed: 0x6d64_3230,
        description: "20 MiB CommonMark + GFM: front matter, headings, lists, task lists, tables, code, math, Mermaid, footnotes",
        generate: text::markdown,
    },
    Fixture {
        name: "docx-1000p",
        file: "report-1000-pages.docx",
        seed: 0x646f_6378,
        description: "1,000-page DOCX: headings, paragraphs, tables and inline images, footer page numbers; each page starts with page-break-before",
        generate: office::docx,
    },
    Fixture {
        name: "pptx-500s",
        file: "deck-500-slides.pptx",
        seed: 0x7070_7478,
        description: "500-slide PPTX: title, bullets and a unique ~1 MB JPEG per slide (~500 MB of media)",
        generate: office::pptx,
    },
    Fixture {
        name: "pdf-5000p",
        file: "book-5000-pages.pdf",
        seed: 0x7064_6635,
        description: "5,000-page, ~1 GB PDF: text plus a unique JPEG per page, outline every 50 pages (not linearized; validate.ps1 makes the linearized copy)",
        generate: pdf::book,
    },
    Fixture {
        name: "pdf-scanned-1000p",
        file: "scanned-1000-pages.pdf",
        seed: 0x7363_616e,
        description: "1,000-page scanned PDF: one 200 dpi grayscale JPEG per page, no text layer",
        generate: pdf::scanned,
    },
];

fn sha256(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::with_capacity(8 << 20, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn main() -> io::Result<()> {
    let mut out = PathBuf::from(".cache/fixtures");
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().expect("--out needs a directory")),
            "--probe-jpeg" => {
                // Encoded sizes for tuning: --probe-jpeg W H NOISE QUALITY
                let n: Vec<u32> = (0..4)
                    .map(|_| args.next().unwrap().parse().unwrap())
                    .collect();
                let sizes: Vec<usize> = (0..4)
                    .map(|i| images::photo_jpeg(1, i, n[0], n[1], n[2] as u8, n[3] as u8).len())
                    .collect();
                println!("{sizes:?}");
                return Ok(());
            }
            "--list" => {
                for f in FIXTURES {
                    println!("{:20} {}", f.name, f.description);
                }
                return Ok(());
            }
            name => names.push(name.to_owned()),
        }
    }
    for n in &names {
        if !FIXTURES.iter().any(|f| f.name == n) {
            eprintln!("unknown fixture {n}; use --list");
            std::process::exit(2);
        }
    }
    std::fs::create_dir_all(&out)?;

    let manifest_path = out.join("manifest.json");
    let mut manifest: serde_json::Value = std::fs::read(&manifest_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    manifest["generatorVersion"] = GENERATOR_VERSION.into();

    for f in FIXTURES
        .iter()
        .filter(|f| names.is_empty() || names.iter().any(|n| n == f.name))
    {
        let path = out.join(f.file);
        eprintln!("generating {} -> {}", f.name, path.display());
        let started = Instant::now();
        let details = (f.generate)(&path, f.seed)?;
        let bytes = std::fs::metadata(&path)?.len();
        let hash = sha256(&path)?;
        eprintln!(
            "  {} bytes in {:.1} s, sha256 {}",
            bytes,
            started.elapsed().as_secs_f64(),
            &hash[..16]
        );
        manifest["fixtures"][f.name] = serde_json::json!({
            "file": f.file,
            "bytes": bytes,
            "sha256": hash,
            "seed": f.seed,
            "description": f.description,
            "details": details,
        });
    }
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    eprintln!("manifest: {}", manifest_path.display());
    Ok(())
}
