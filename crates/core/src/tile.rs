//! Tile addressing and the source trait the view service renders from
//! (MASTER_PROMPT §5.4; ADR-0004, ADR-0005).
//!
//! A view's content is laid out in document space: device pixels at the
//! view's zoom, with `y` growing down from the top of the document. Tiles
//! are square, `size` device px on a side, and tile `(col, row)` covers
//! `[col·size, (col+1)·size) × [row·size, (row+1)·size)`. Rows are `u64` and
//! positions `f64`, so documents far taller than a browser element can be
//! (e.g. 10M rows × 30 px = 3·10^8 px) are addressed exactly.

use serde::{Deserialize, Serialize};

use crate::scheduler::{Interrupted, JobContext, JobKey};

/// Identifies one tile of one layout of one view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileKey {
    pub view: u64,
    /// Layout revision: changes when content or layout width changes, so
    /// tiles from an older layout are never mixed with new ones.
    pub layout: u64,
    pub col: u32,
    pub row: u64,
    /// Edge length in device pixels.
    pub size: u32,
}

impl TileKey {
    /// Scheduler key: requests for the same tile coalesce.
    pub fn job_key(&self) -> JobKey {
        JobKey {
            kind: "tile",
            parts: [
                self.view,
                self.layout,
                (u64::from(self.col) << 32) | u64::from(self.size),
                self.row,
            ],
        }
    }

    /// Document-space position of the tile's top-left corner.
    pub fn origin(&self) -> (f64, f64) {
        let s = f64::from(self.size);
        (f64::from(self.col) * s, self.row as f64 * s)
    }
}

/// Size of a laid-out document, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extent {
    pub width: f64,
    pub height: f64,
    /// False while the size is still an estimate (§7: progressive
    /// everything); the UI labels it as such.
    pub exact: bool,
}

/// A rendered tile: tightly packed, opaque RGBA8.
#[derive(Clone, Debug)]
pub struct RenderedTile {
    pub key: TileKey,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Something a view can render tiles from.
pub trait TileSource: Send + Sync {
    /// Lays the content out for a viewport `width` device px wide and returns
    /// the layout id and its extent.
    fn layout(&self, width: u32) -> (u64, Extent);

    /// Renders one tile. Long renders should call `ctx.checkpoint()?`.
    fn render(&self, key: &TileKey, ctx: &JobContext) -> Result<RenderedTile, Interrupted>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_origin_is_exact_far_down_a_document() {
        let key = TileKey {
            view: 1,
            layout: 1,
            col: 3,
            row: 585_937, // ~3·10^8 px at 512 px tiles
            size: 512,
        };
        assert_eq!(key.origin(), (1536.0, 299_999_744.0));
    }

    #[test]
    fn distinct_tiles_have_distinct_job_keys() {
        let a = TileKey {
            view: 1,
            layout: 2,
            col: 0,
            row: 5,
            size: 512,
        };
        let variants = [
            TileKey { col: 1, ..a },
            TileKey { row: 6, ..a },
            TileKey { size: 256, ..a },
            TileKey { layout: 3, ..a },
            TileKey { view: 2, ..a },
        ];
        for v in variants {
            assert_ne!(a.job_key(), v.job_key());
        }
    }
}
