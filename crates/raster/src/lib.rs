//! CPU rasterization into tiles (MASTER_PROMPT §5.4; ADR-0006 baseline).
//!
//! A [`TileCanvas`] is one tile's pixel buffer in device pixels with a
//! document-space origin, so callers draw in document coordinates and the
//! canvas clips to its tile. Output is RGBA8; tiles are opaque, so the
//! premultiplied pixels tiny-skia produces are also straight RGBA.

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// An opaque sRGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    fn paint(self) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(self.0, self.1, self.2, 255));
        paint.anti_alias = true;
        paint
    }
}

/// One tile being drawn.
pub struct TileCanvas {
    pixmap: Pixmap,
    /// Document-space position of the tile's top-left corner.
    origin: (f64, f64),
}

impl std::fmt::Debug for TileCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileCanvas")
            .field("size", &(self.width(), self.height()))
            .field("origin", &self.origin)
            .finish()
    }
}

impl TileCanvas {
    /// A `width`×`height` tile whose top-left is at `origin` in document
    /// space, filled with `background`.
    pub fn new(width: u32, height: u32, origin: (f64, f64), background: Rgb) -> Option<Self> {
        let mut pixmap = Pixmap::new(width, height)?;
        pixmap.fill(Color::from_rgba8(
            background.0,
            background.1,
            background.2,
            255,
        ));
        Some(Self { pixmap, origin })
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// The tile's area in document space.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let (x, y) = self.origin;
        (
            x,
            y,
            x + f64::from(self.width()),
            y + f64::from(self.height()),
        )
    }

    /// Converts a document-space rectangle to tile space. Precision note:
    /// document offsets are f64 (exact to 2^53), and only the small
    /// tile-relative result is narrowed to f32.
    fn local_rect(&self, x: f64, y: f64, w: f64, h: f64) -> Option<Rect> {
        Rect::from_xywh(
            (x - self.origin.0) as f32,
            (y - self.origin.1) as f32,
            w as f32,
            h as f32,
        )
    }

    /// Fills a document-space rectangle.
    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, color: Rgb) {
        if let Some(rect) = self.local_rect(x, y, w, h) {
            self.pixmap
                .fill_rect(rect, &color.paint(), Transform::identity(), None);
        }
    }

    /// Fills a closed polygon given in document-space points.
    pub fn fill_polygon(&mut self, points: &[(f64, f64)], color: Rgb) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            let (lx, ly) = ((x - self.origin.0) as f32, (y - self.origin.1) as f32);
            if i == 0 {
                pb.move_to(lx, ly);
            } else {
                pb.line_to(lx, ly);
            }
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(
                &path,
                &color.paint(),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// The finished tile as tightly packed RGBA8 rows.
    pub fn into_rgba(self) -> Vec<u8> {
        self.pixmap.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_in_document_space_and_clips_to_the_tile() {
        // Tile covering document rows 1_000_000_000..1_000_000_004.
        let mut t = TileCanvas::new(4, 4, (0.0, 1e9), Rgb(255, 255, 255)).unwrap();
        t.fill_rect(1.0, 1e9 + 1.0, 2.0, 2.0, Rgb(10, 20, 30));
        t.fill_rect(0.0, 0.0, 4.0, 4.0, Rgb(0, 0, 0)); // far above: clipped away
        let px = t.into_rgba();
        let at = |x: usize, y: usize| &px[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(0, 0), [255, 255, 255, 255]);
        assert_eq!(at(1, 1), [10, 20, 30, 255]);
        assert_eq!(at(2, 2), [10, 20, 30, 255]);
        assert_eq!(at(3, 3), [255, 255, 255, 255]);
    }

    #[test]
    fn polygons_are_filled() {
        let mut t = TileCanvas::new(8, 8, (100.0, 100.0), Rgb(0, 0, 0)).unwrap();
        t.fill_polygon(
            &[
                (100.0, 100.0),
                (108.0, 100.0),
                (108.0, 108.0),
                (100.0, 108.0),
            ],
            Rgb(1, 2, 3),
        );
        assert!(t.into_rgba().chunks(4).all(|p| p == [1, 2, 3, 255]));
    }
}
