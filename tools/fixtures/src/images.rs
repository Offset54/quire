//! Synthetic images. Each image is seeded by (fixture seed, index), so they
//! can be generated in parallel and still be byte-identical between runs.

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::rng::Rng;

fn image_rng(seed: u64, index: u64) -> Rng {
    Rng::new(seed ^ index.wrapping_mul(0xA24B_AED4_963E_E407))
}

/// Generates `count` items in parallel, in index order.
pub fn par_generate<T: Send>(count: u64, make: impl Fn(u64) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    let chunk = count.div_ceil(threads).max(1) as usize;
    std::thread::scope(|s| {
        for (c, slots) in out.chunks_mut(chunk).enumerate() {
            let make = &make;
            s.spawn(move || {
                for (k, slot) in slots.iter_mut().enumerate() {
                    *slot = Some(make((c * chunk + k) as u64));
                }
            });
        }
    });
    out.into_iter().map(|v| v.expect("generated")).collect()
}

/// A photo-like RGB JPEG: smooth colour field, blobs and per-pixel noise.
/// `noise` (0–255) is the main knob for the encoded size.
pub fn photo_jpeg(seed: u64, index: u64, w: u32, h: u32, noise: u8, quality: u8) -> Vec<u8> {
    let mut rng = image_rng(seed, index);
    let (cr, cg, cb) = (rng.below(256), rng.below(256), rng.below(256));
    let blobs: Vec<(f64, f64, f64, [u8; 3])> = (0..6)
        .map(|_| {
            (
                rng.below(u64::from(w)) as f64,
                rng.below(u64::from(h)) as f64,
                (rng.range(40, 300)) as f64,
                [
                    rng.below(256) as u8,
                    rng.below(256) as u8,
                    rng.below(256) as u8,
                ],
            )
        })
        .collect();
    let mut px = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let fx = f64::from(x) / f64::from(w);
            let fy = f64::from(y) / f64::from(h);
            let mut c = [
                (cr as f64 * (1.0 - fx) + 255.0 * fx * 0.3),
                (cg as f64 * (1.0 - fy) + 128.0 * fy),
                (cb as f64 * 0.5 + 127.0 * (fx * fy)),
            ];
            for &(bx, by, r, col) in &blobs {
                let d2 = (f64::from(x) - bx).powi(2) + (f64::from(y) - by).powi(2);
                if d2 < r * r {
                    let t = 1.0 - d2.sqrt() / r;
                    for k in 0..3 {
                        c[k] = c[k] * (1.0 - t) + f64::from(col[k]) * t;
                    }
                }
            }
            let n = rng.next_u64();
            let i = ((y * w + x) * 3) as usize;
            for k in 0..3 {
                let jitter = ((n >> (k * 8)) & 0xff) as f64 / 255.0 - 0.5;
                px[i + k] = (c[k] + jitter * f64::from(noise)).clamp(0.0, 255.0) as u8;
            }
        }
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode(&px, w, h, ExtendedColorType::Rgb8)
        .expect("jpeg encode");
    out
}

/// A scanned-page look: grey paper with grain and a slight shading
/// gradient, dark text-like marks in lines, as an 8-bit grayscale JPEG.
pub fn scan_jpeg(seed: u64, index: u64, w: u32, h: u32) -> Vec<u8> {
    let mut rng = image_rng(seed, index);
    let mut px = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let shade =
                238.0 - 10.0 * (f64::from(x) / f64::from(w)) - 6.0 * (f64::from(y) / f64::from(h));
            let grain = (rng.next_u64() & 0x0f) as f64 - 8.0;
            px[(y * w + x) as usize] = (shade + grain).clamp(0.0, 255.0) as u8;
        }
    }
    // Text-like marks: lines of "words" made of dark blocks.
    let margin = w / 10;
    let line_h = h / 60;
    let mut y = h / 12;
    while y + line_h < h - h / 12 {
        let mut x = margin;
        let end = w - margin - rng.below(u64::from(w / 4)) as u32;
        while x < end {
            let word = rng.range(3, 10) as u32 * (line_h / 3);
            for yy in y..y + line_h * 2 / 3 {
                for xx in x..(x + word).min(end) {
                    let v = 40 + (rng.next_u64() & 0x3f) as u8;
                    px[(yy * w + xx) as usize] = v;
                }
            }
            x += word + line_h / 2;
        }
        y += line_h + line_h / 2 + if rng.chance(60) { line_h * 2 } else { 0 };
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 70)
        .encode(&px, w, h, ExtendedColorType::L8)
        .expect("jpeg encode");
    out
}

/// A bar-chart-like RGBA PNG (flat colours, compresses well).
pub fn chart_png(seed: u64, index: u64, w: u32, h: u32) -> Vec<u8> {
    let mut rng = image_rng(seed, index);
    let mut px = vec![255u8; (w * h * 4) as usize];
    let bars = rng.range(4, 12) as u32;
    let bw = w / (bars * 2 + 1);
    for b in 0..bars {
        let bh = rng.range(u64::from(h / 8), u64::from(h * 7 / 8)) as u32;
        let colour = [
            rng.below(200) as u8,
            rng.below(200) as u8,
            rng.below(200) as u8,
        ];
        let x0 = bw * (2 * b + 1);
        for y in (h - bh)..h {
            for x in x0..x0 + bw {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&colour);
            }
        }
    }
    for x in 0..w {
        let i = (((h - 1) * w + x) * 4) as usize;
        px[i..i + 3].copy_from_slice(&[0, 0, 0]);
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&px, w, h, ExtendedColorType::Rgba8)
        .expect("png encode");
    out
}
