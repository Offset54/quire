//! Reference-machine emulation helper (ADR-0021; M0 task 12).
//!
//! ```text
//! refsim info                         CPU sets, memory, power, as JSON
//! refsim run [--pcores] [--logical N] -- <program> [args...]
//!                                     run a program (and every process it starts)
//!                                     pinned to the performance cores
//! refsim purge <file>...              evict files from the OS file cache
//! refsim seqread <file>               sequential read throughput, MB/s
//! refsim ballast <GiB> <seconds>      hold memory committed and resident
//! refsim calibrate                    fixed CPU kernel suite, as JSON
//! ```
//!
//! Nothing here changes a system setting: pinning uses a Job Object that
//! only affects the processes inside it.

mod calibrate;
#[cfg(windows)]
mod win;

use std::io::Read;
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "usage: refsim info | run [--pcores] [--logical N] -- <program> [args...] | purge <file>... | seqread <file> | seqwrite <file> <bytes> | ballast <GiB> <seconds> | calibrate"
    );
    std::process::exit(2)
}

/// Sequential read with an 8 MiB buffer; returns MB/s (10^6 bytes).
/// Sequential write of `bytes` with an 8 MiB buffer, then fsync; the file
/// is deleted afterwards. The baseline for "saving is limited only by
/// sequential write speed" (§4).
fn seqwrite(path: &str, bytes: u64) -> std::io::Result<serde_json::Value> {
    use std::io::Write;
    let buf: Vec<u8> = (0..8usize << 20).map(|i| b'a' + (i % 26) as u8).collect();
    let start = Instant::now();
    let mut f = std::fs::File::create(path)?;
    let mut left = bytes;
    while left > 0 {
        let n = (buf.len() as u64).min(left) as usize;
        f.write_all(&buf[..n])?;
        left -= n as u64;
    }
    f.sync_all()?;
    let secs = start.elapsed().as_secs_f64();
    drop(f);
    std::fs::remove_file(path)?;
    Ok(serde_json::json!({
        "file": path, "bytes": bytes, "seconds": secs, "mbPerSecond": bytes as f64 / 1e6 / secs
    }))
}

fn seqread(path: &str) -> std::io::Result<serde_json::Value> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 8 << 20];
    let start = Instant::now();
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
    }
    let secs = start.elapsed().as_secs_f64();
    Ok(serde_json::json!({
        "file": path, "bytes": total, "seconds": secs, "mbPerSecond": total as f64 / 1e6 / secs
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let result: Result<serde_json::Value, String> = match cmd.as_str() {
        "calibrate" => Ok(calibrate::run()),
        "seqwrite" => match (args.get(1), args.get(2).and_then(|b| b.parse().ok())) {
            (Some(p), Some(b)) => seqwrite(p, b).map_err(|e| e.to_string()),
            _ => usage(),
        },
        "seqread" => args
            .get(1)
            .map_or_else(|| usage(), |p| seqread(p).map_err(|e| e.to_string())),
        #[cfg(windows)]
        "info" => win::info(),
        #[cfg(windows)]
        "purge" => win::purge(&args[1..]),
        #[cfg(windows)]
        "ballast" => {
            let gib: f64 = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| usage());
            let secs: u64 = args
                .get(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| usage());
            win::ballast(gib, secs)
        }
        #[cfg(windows)]
        "run" => {
            let mut pcores = false;
            let mut logical = None;
            let mut rest = args[1..].iter();
            let mut program = Vec::new();
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--pcores" => pcores = true,
                    "--logical" => logical = rest.next().and_then(|s| s.parse().ok()),
                    "--" => {
                        program = rest.cloned().collect();
                        break;
                    }
                    _ => usage(),
                }
            }
            if program.is_empty() {
                usage()
            }
            match win::run(&program, pcores, logical) {
                Ok(code) => std::process::exit(code),
                Err(e) => Err(e),
            }
        }
        #[cfg(not(windows))]
        "info" | "purge" | "ballast" | "run" => Err("this command is Windows-only".into()),
        _ => usage(),
    };
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            eprintln!("refsim: {e}");
            std::process::exit(1);
        }
    }
}
