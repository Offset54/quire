//! Synthetic list document for the M0 scroll benchmark (DoD: "a synthetic
//! 10M-row list scrolls at 60 fps"). Compiled only with the `bench-fixtures`
//! feature; never reachable from the shipped UI.
//!
//! Each row shows its index as seven-segment digits (no font stack is needed
//! yet) and a few bars of row-dependent width, so every tile is distinct and
//! a wrong or stale tile is visible on screen.

use quire_core::scheduler::{Interrupted, JobContext};
use quire_core::tile::{Extent, RenderedTile, TileKey, TileSource};
use quire_raster::{Rgb, TileCanvas};

pub struct SyntheticList {
    rows: u64,
    row_height: f64,
}

const WHITE: Rgb = Rgb(255, 255, 255);
const ZEBRA: Rgb = Rgb(244, 245, 248);
const RULE: Rgb = Rgb(226, 228, 235);
const INK: Rgb = Rgb(40, 44, 60);
const BAR: Rgb = Rgb(150, 156, 180);

impl SyntheticList {
    pub fn new(rows: u64, row_height: f64) -> Self {
        Self {
            rows,
            row_height: row_height.max(4.0),
        }
    }

    fn draw_row(&self, canvas: &mut TileCanvas, index: u64, width: f64) {
        let h = self.row_height;
        let y = index as f64 * h;
        canvas.fill_rect(
            0.0,
            y,
            width,
            h,
            if index.is_multiple_of(2) {
                WHITE
            } else {
                ZEBRA
            },
        );
        canvas.fill_rect(0.0, y + h - 1.0, width, 1.0, RULE);

        let digit_h = (h * 0.55).round();
        let mut x = h * 0.5;
        for ch in index.to_string().bytes() {
            draw_digit(canvas, ch - b'0', x, y + (h - digit_h) / 2.0, digit_h);
            x += digit_h * 0.62;
        }

        // Row-dependent "cells".
        let mut seed = index.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut cx = h * 7.0;
        for _ in 0..5 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let w = h * (1.5 + (seed % 60) as f64 / 10.0);
            if cx + w > width {
                break;
            }
            canvas.fill_rect(cx, y + h * 0.35, w, h * 0.3, BAR);
            cx += w + h;
        }
    }
}

/// Seven-segment digit: segments a–g, top-left at (x, y), height `h`.
fn draw_digit(canvas: &mut TileCanvas, digit: u8, x: f64, y: f64, h: f64) {
    const SEGMENTS: [u8; 10] = [
        0b011_1111, 0b000_0110, 0b101_1011, 0b100_1111, 0b110_0110, 0b110_1101, 0b111_1101,
        0b000_0111, 0b111_1111, 0b110_1111,
    ];
    let w = h * 0.5;
    let t = (h / 8.0).max(1.0);
    let half = h / 2.0;
    let rects = [
        (x, y, w, t),                   // a
        (x + w - t, y, t, half),        // b
        (x + w - t, y + half, t, half), // c
        (x, y + h - t, w, t),           // d
        (x, y + half, t, half),         // e
        (x, y, t, half),                // f
        (x, y + half - t / 2.0, w, t),  // g
    ];
    let mask = SEGMENTS[usize::from(digit.min(9))];
    for (i, &(rx, ry, rw, rh)) in rects.iter().enumerate() {
        if mask & (1 << i) != 0 {
            canvas.fill_rect(rx, ry, rw, rh, INK);
        }
    }
}

impl TileSource for SyntheticList {
    fn layout(&self, width: u32) -> (u64, Extent) {
        (
            u64::from(width),
            Extent {
                width: f64::from(width),
                height: self.rows as f64 * self.row_height,
                exact: true,
            },
        )
    }

    fn render(&self, key: &TileKey, ctx: &JobContext) -> Result<RenderedTile, Interrupted> {
        let layout_width = key.layout as f64;
        let mut canvas = TileCanvas::new(key.size, key.size, key.origin(), WHITE)
            .expect("tile size is non-zero");
        let (_, top, _, bottom) = canvas.bounds();
        let first = (top / self.row_height).floor() as u64;
        let last = ((bottom / self.row_height).ceil() as u64).min(self.rows);
        for index in first..last {
            ctx.checkpoint()?;
            self.draw_row(&mut canvas, index, layout_width);
        }
        let (width, height) = (canvas.width(), canvas.height());
        Ok(RenderedTile {
            key: *key,
            width,
            height,
            rgba: canvas.into_rgba(),
        })
    }
}
