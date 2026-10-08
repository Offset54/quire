//! Synthetic RGBA tiles and their encodings.
//!
//! - `Text`: white page with dark, antialiased glyph-like marks in lines,
//!   which is what most document tiles look like (very compressible).
//! - `Photo`: smooth gradient plus per-pixel noise (close to incompressible),
//!   the worst case for compression.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Raw,
    Lz4,
    Qoi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Content {
    Text,
    Photo,
}

pub const SIZES: [u32; 2] = [256, 512];
/// Distinct tiles per (content, size), so caches can't serve repeats.
const VARIANTS: u64 = 16;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic, good enough for test patterns.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn text_tile(size: u32, seed: u64) -> Vec<u8> {
    let n = size as usize;
    let mut px = vec![255u8; n * n * 4];
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let line_height = 27; // ~11 pt at 150 %
    let mut y = 6;
    while y + line_height <= n {
        let mut x = 4 + rng.below(8) as usize;
        let line_end = n - rng.below(n as u64 / 4) as usize;
        while x + 10 < line_end {
            let w = 5 + rng.below(6) as usize;
            let h = 11 + rng.below(6) as usize;
            let top = y + (16 - h.min(16));
            for gy in top..top + h {
                for gx in x..x + w {
                    // Darker core, lighter edges (antialiasing).
                    let edge = gx == x || gx == x + w - 1 || gy == top || gy == top + h - 1;
                    let v = if edge { 150 } else { 35 };
                    let i = (gy * n + gx) * 4;
                    px[i..i + 3].copy_from_slice(&[v, v, v]);
                }
            }
            x += w + 1 + if rng.below(6) == 0 { 7 } else { 0 };
        }
        y += line_height;
    }
    px
}

fn photo_tile(size: u32, seed: u64) -> Vec<u8> {
    let n = size as usize;
    let mut px = vec![255u8; n * n * 4];
    let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    for y in 0..n {
        for x in 0..n {
            let noise = rng.next();
            let i = (y * n + x) * 4;
            px[i] = ((x * 255 / n) as u8).wrapping_add((noise & 31) as u8);
            px[i + 1] = ((y * 255 / n) as u8).wrapping_add((noise >> 8 & 31) as u8);
            px[i + 2] = (((x + y) * 127 / n) as u8).wrapping_add((noise >> 16 & 31) as u8);
        }
    }
    px
}

type TileSet = Vec<Arc<Vec<u8>>>;

/// Every tile variant in every encoding, generated once at startup.
pub struct Store {
    tiles: HashMap<(Encoding, Content, u32), TileSet>,
}

impl Store {
    pub fn generate() -> Self {
        let mut tiles = HashMap::new();
        for size in SIZES {
            for content in [Content::Text, Content::Photo] {
                let raw: Vec<Vec<u8>> = (0..VARIANTS)
                    .map(|v| match content {
                        Content::Text => text_tile(size, v + 1),
                        Content::Photo => photo_tile(size, v + 1),
                    })
                    .collect();
                let lz4 = raw
                    .iter()
                    .map(|r| Arc::new(lz4_flex::block::compress_prepend_size(r)))
                    .collect();
                let qoi = raw
                    .iter()
                    .map(|r| Arc::new(qoi::encode_to_vec(r, size, size).expect("qoi encode")))
                    .collect();
                tiles.insert((Encoding::Lz4, content, size), lz4);
                tiles.insert((Encoding::Qoi, content, size), qoi);
                tiles.insert(
                    (Encoding::Raw, content, size),
                    raw.into_iter().map(Arc::new).collect(),
                );
            }
        }
        Self { tiles }
    }

    pub fn get(&self, encoding: Encoding, content: Content, size: u32, i: u64) -> Arc<Vec<u8>> {
        let set = &self.tiles[&(encoding, content, size)];
        Arc::clone(&set[(i % set.len() as u64) as usize])
    }

    /// Average encoded size per tile, for the report.
    pub fn sizes(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for ((encoding, content, size), set) in &self.tiles {
            let avg = set.iter().map(|t| t.len()).sum::<usize>() / set.len();
            out.push(serde_json::json!({
                "encoding": encoding, "content": content, "size": size, "avgBytes": avg
            }));
        }
        out
    }
}
