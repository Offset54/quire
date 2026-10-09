//! Transcoding throughput for ADR-0009.
//!
//! - `cargo run -p quire-text --release --example transcodebench -- gen <dir> <MiB>`
//!   writes deterministic Shift_JIS, EUC-KR (CP949), windows-1252 and
//!   UTF-16LE files of about `<MiB>` each into `<dir>`.
//! - `... -- open <file> [threads]` opens one the way the app does
//!   (detection, then parallel transcoding to UTF-8) and prints JSON.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use encoding_rs::{EUC_KR, Encoding, SHIFT_JIS, UTF_16LE, WINDOWS_1252};
use quire_core::memory::{GovernorConfig, MemoryGovernor};
use quire_text::file::{OpenSettings, open};

/// Lines in the language each encoding is used for, with some ASCII.
fn sample(e: &'static Encoding) -> Vec<&'static str> {
    if e == SHIFT_JIS {
        vec![
            "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。",
            "東京都千代田区丸の内一丁目 ９番１号 ＪＲ東日本 2024年版 カタカナ ｶﾀｶﾅ",
            "第3章: 文字コードの変換と行インデックス (Shift_JIS) 表示・編集・保存",
        ]
    } else if e == EUC_KR {
        vec![
            "모든 사람은 태어날 때부터 자유로우며 그 존엄과 권리에 있어 동등하다.",
            "서울특별시 중구 세종대로 110, 전화 02-120 (평일 09:00~18:00)",
            "제3장: 문자 인코딩 변환과 줄 색인 (CP949) 보기, 편집, 저장",
        ]
    } else if e == WINDOWS_1252 {
        vec![
            "Größe, Straße, façade, naïve, déjà vu, señor, smørrebrød, Œuvre — 12,50 €",
            "The quick brown fox jumps over the lazy dog. «Citation» ‘quoted’ “text”…",
            "Chapter 3: transcoding and the line index (windows-1252) ± ½ ¼ ¾ © ®",
        ]
    } else {
        vec![
            "Mixed: English, Ελληνικά, Русский, 日本語, 한국어, עברית, العربية 🙂",
            "UTF-16 text with surrogate pairs 𝒜𝓁𝓅𝒽𝒶 and emoji 👍🏽 and CJK 漢字",
            "Chapter 3: transcoding and the line index (UTF-16LE) ✓",
        ]
    }
}

fn generate(dir: &Path, mib: u64) {
    std::fs::create_dir_all(dir).unwrap();
    for (e, name) in [
        (SHIFT_JIS, "sjis"),
        (EUC_KR, "cp949"),
        (WINDOWS_1252, "cp1252"),
        (UTF_16LE, "utf16le"),
    ] {
        let path = dir.join(format!("legacy-{name}-{mib}mib.txt"));
        let mut w =
            std::io::BufWriter::with_capacity(4 << 20, std::fs::File::create(&path).unwrap());
        let lines = sample(e);
        let target = mib << 20;
        let mut written = 0u64;
        let mut i = 0u64;
        if e == UTF_16LE {
            w.write_all(&[0xFF, 0xFE]).unwrap(); // BOM
            written += 2;
        }
        while written < target {
            let line = format!("{i:>9}| {}\r\n", lines[(i % lines.len() as u64) as usize]);
            let bytes: Vec<u8> = if e == UTF_16LE {
                line.encode_utf16().flat_map(u16::to_le_bytes).collect()
            } else {
                let (b, _, unmappable) = e.encode(&line);
                assert!(!unmappable, "{name}: unmappable text in the sample");
                b.into_owned()
            };
            w.write_all(&bytes).unwrap();
            written += bytes.len() as u64;
            i += 1;
        }
        w.flush().unwrap();
        println!("{} {} bytes, {i} lines", path.display(), written);
    }
}

fn open_once(path: &Path, threads: usize) {
    let g = MemoryGovernor::new(GovernorConfig::default());
    let size = std::fs::metadata(path).unwrap().len();
    let t = Instant::now();
    let f = open(
        path,
        &g,
        None,
        &OpenSettings {
            encoding: None,
            temp_dir: std::env::temp_dir(),
            threads,
            progress: &|_| true,
        },
    )
    .unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!(
        "{{\"file\":{:?},\"encoding\":\"{}\",\"threads\":{threads},\"rawBytes\":{size},\"utf8Bytes\":{},\"seconds\":{secs:.4},\"rawMBps\":{:.1}}}",
        path.display().to_string(),
        f.detection.encoding.name(),
        f.doc.len(),
        size as f64 / 1e6 / secs
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") => generate(Path::new(&args[2]), args[3].parse().unwrap()),
        Some("open") => open_once(
            Path::new(&args[2]),
            args.get(3).map_or(4, |t| t.parse().unwrap()),
        ),
        _ => eprintln!("usage: transcodebench gen <dir> <MiB> | open <file> [threads]"),
    }
}
