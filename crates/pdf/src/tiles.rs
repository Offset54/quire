//! Page tiles (§5.4, ADR-0038): each page is cut into square tiles of its
//! own, at a scale (device px per point) and a view rotation. Tiles are
//! page-local, so changing the layout (continuous, single, two-up) moves
//! pages without invalidating a tile.

use crate::protocol::Matrix;

/// View rotation, clockwise, on top of the page's own `/Rotate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(deg: u32) -> Option<Self> {
        match deg % 360 {
            0 => Some(Rotation::R0),
            90 => Some(Rotation::R90),
            180 => Some(Rotation::R180),
            270 => Some(Rotation::R270),
            _ => None,
        }
    }

    pub fn quarter_turns(self) -> u64 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 1,
            Rotation::R180 => 2,
            Rotation::R270 => 3,
        }
    }

    /// The page's size as shown, in points.
    pub fn shown(self, (w, h): (f32, f32)) -> (f32, f32) {
        match self {
            Rotation::R0 | Rotation::R180 => (w, h),
            Rotation::R90 | Rotation::R270 => (h, w),
        }
    }
}

/// The matrix that renders the tile whose top-left corner is at device
/// pixel `(tx, ty)` of a `page` (w × h points) shown at `scale` and rotated
/// by `rot`.
pub fn tile_matrix(page: (f32, f32), rot: Rotation, scale: f32, tx: f32, ty: f32) -> Matrix {
    let (w, h) = page;
    let s = scale;
    // x' = a·x + c·y + e, y' = b·x + d·y + f, from page space (y down).
    let (a, b, c, d, e, f) = match rot {
        Rotation::R0 => (s, 0.0, 0.0, s, 0.0, 0.0),
        // (x, y) → (h − y, x)
        Rotation::R90 => (0.0, s, -s, 0.0, s * h, 0.0),
        // (x, y) → (w − x, h − y)
        Rotation::R180 => (-s, 0.0, 0.0, -s, s * w, s * h),
        // (x, y) → (y, w − x)
        Rotation::R270 => (0.0, -s, s, 0.0, 0.0, s * w),
    };
    Matrix {
        a,
        b,
        c,
        d,
        e: e - tx,
        f: f - ty,
    }
}

/// Device-pixel size of the page as shown, rounded up.
pub fn page_pixels(page: (f32, f32), rot: Rotation, scale: f32) -> (u32, u32) {
    let (w, h) = rot.shown(page);
    (
        (w * scale).ceil().max(1.0) as u32,
        (h * scale).ceil().max(1.0) as u32,
    )
}

/// Pixel size of tile `(col, row)`: `size` square, clipped at the page's
/// right and bottom edges. `None` if the tile lies outside the page.
pub fn tile_size(pixels: (u32, u32), size: u32, col: u32, row: u32) -> Option<(u32, u32)> {
    let x = u64::from(col) * u64::from(size);
    let y = u64::from(row) * u64::from(size);
    let (pw, ph) = (u64::from(pixels.0), u64::from(pixels.1));
    if x >= pw || y >= ph {
        return None;
    }
    Some((
        (pw - x).min(u64::from(size)) as u32,
        (ph - y).min(u64::from(size)) as u32,
    ))
}

/// Tile keys' `layout` field: everything that changes a tile's pixels
/// besides its position. Stays below 2^53 so the frontend holds it exactly.
pub fn layout_id(scale: f32, rot: Rotation, revision: u32) -> u64 {
    (u64::from(revision & 0x7_FFFF) << 34) | (u64::from(scale.to_bits()) << 2) | rot.quarter_turns()
}

/// Tile keys' `row` field: the page and the tile row within it.
pub fn row_id(page: u32, row: u32) -> u64 {
    (u64::from(page) << 24) | u64::from(row & 0xFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Matrix, (x, y): (f32, f32)) -> (f32, f32) {
        (m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f)
    }

    #[test]
    fn rotations_map_the_page_onto_its_shown_box() {
        let page = (600.0, 800.0);
        for (deg, tl, tr) in [
            // Where the page's top-left and top-right corners land, at 1 px/pt.
            (0, (0.0, 0.0), (600.0, 0.0)),
            (90, (800.0, 0.0), (800.0, 600.0)),
            (180, (600.0, 800.0), (0.0, 800.0)),
            (270, (0.0, 600.0), (0.0, 0.0)),
        ] {
            let rot = Rotation::from_degrees(deg).unwrap();
            let m = tile_matrix(page, rot, 1.0, 0.0, 0.0);
            assert_eq!(apply(&m, (0.0, 0.0)), tl, "{deg}°");
            assert_eq!(apply(&m, (600.0, 0.0)), tr, "{deg}°");
            // Every corner stays inside the shown box.
            let (sw, sh) = rot.shown(page);
            for p in [(0.0, 0.0), (600.0, 0.0), (0.0, 800.0), (600.0, 800.0)] {
                let (x, y) = apply(&m, p);
                assert!(
                    (0.0..=sw).contains(&x) && (0.0..=sh).contains(&y),
                    "{deg}° {p:?}"
                );
            }
        }
    }

    #[test]
    fn tiles_translate_by_their_origin() {
        let m = tile_matrix((600.0, 800.0), Rotation::R0, 2.0, 512.0, 1024.0);
        assert_eq!(apply(&m, (256.0, 512.0)), (0.0, 0.0));
    }

    #[test]
    fn edge_tiles_are_clipped() {
        let px = page_pixels((595.0, 842.0), Rotation::R0, 2.0);
        assert_eq!(px, (1190, 1684));
        assert_eq!(tile_size(px, 512, 0, 0), Some((512, 512)));
        assert_eq!(tile_size(px, 512, 2, 3), Some((166, 148)));
        assert_eq!(tile_size(px, 512, 3, 0), None);
        assert_eq!(page_pixels((595.0, 842.0), Rotation::R90, 1.0), (842, 595));
    }

    #[test]
    fn ids_stay_exact_in_a_double() {
        let l = layout_id(f32::MAX, Rotation::R270, u32::MAX);
        assert!(l < 1 << 53);
        assert_ne!(
            layout_id(2.0, Rotation::R0, 0),
            layout_id(2.0, Rotation::R90, 0)
        );
        assert_ne!(
            layout_id(2.0, Rotation::R0, 0),
            layout_id(2.0, Rotation::R0, 1)
        );
        assert!(row_id(u32::MAX >> 3, 0xFF_FFFF) < 1 << 53);
        assert_eq!(row_id(3, 5), (3 << 24) | 5);
    }
}
