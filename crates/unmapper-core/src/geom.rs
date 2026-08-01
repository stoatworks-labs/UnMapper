//! Rectangles and quads.
//!
//! Everything geometric in UnMapper is a **quad**, not a rect. Resolume lets
//! a slice be rotated and corner-pinned, and a slice that has been corner-pinned
//! samples a genuine trapezoid out of its source. Collapsing that to a bounding
//! box — which is all you need if you are only *drawing outlines*, and is what
//! the sibling importer in `test-card` does — would sample the wrong texels here,
//! because these coordinates feed a shader rather than a stroke.
//!
//! So `Quad` is the primitive and `Rect` is the convenience.

use serde::{Deserialize, Serialize};

pub use glam::{Vec2, Vec3};

/// An axis-aligned rectangle, origin top-left, in whatever space the holder says.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rect from `(0,0)` to `(w,h)`.
    pub const fn from_size(width: f32, height: f32) -> Self {
        Self::new(0.0, 0.0, width, height)
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn area(&self) -> f32 {
        self.width * self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// The overlap of two rects, or `None` when they do not touch.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// The smallest rect containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Map a point in this rect to `0..1` across it.
    pub fn normalize(&self, p: Vec2) -> Vec2 {
        Vec2::new((p.x - self.x) / self.width, (p.y - self.y) / self.height)
    }
}

/// Four corners, in the order top-left, top-right, bottom-right, bottom-left.
///
/// "Top-left" names the corner's role in the *source* image, not its position on
/// screen: rotate a slice 180 degrees and `tl` is the one at the bottom right.
/// Keeping the winding fixed is what lets a rotation survive the round trip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quad {
    pub tl: Vec2,
    pub tr: Vec2,
    pub br: Vec2,
    pub bl: Vec2,
}

impl Quad {
    pub const fn new(tl: Vec2, tr: Vec2, br: Vec2, bl: Vec2) -> Self {
        Self { tl, tr, br, bl }
    }

    /// The axis-aligned quad covering `rect`.
    pub fn from_rect(rect: Rect) -> Self {
        Self {
            tl: Vec2::new(rect.x, rect.y),
            tr: Vec2::new(rect.right(), rect.y),
            br: Vec2::new(rect.right(), rect.bottom()),
            bl: Vec2::new(rect.x, rect.bottom()),
        }
    }

    pub fn corners(&self) -> [Vec2; 4] {
        [self.tl, self.tr, self.br, self.bl]
    }

    /// The bounding box. Lossy for anything rotated — use it for hit-testing and
    /// culling, never for sampling.
    pub fn bounds(&self) -> Rect {
        let c = self.corners();
        let x0 = c.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let y0 = c.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let x1 = c.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let y1 = c.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Whether this quad is an unrotated rectangle, within `eps` pixels.
    ///
    /// The bounding box is exact only for these, so this is the test for whether
    /// a slice can take the cheap blit path instead of the warp path.
    pub fn is_axis_aligned(&self, eps: f32) -> bool {
        (self.tl.y - self.tr.y).abs() <= eps
            && (self.bl.y - self.br.y).abs() <= eps
            && (self.tl.x - self.bl.x).abs() <= eps
            && (self.tr.x - self.br.x).abs() <= eps
    }

    /// Bilinear interpolation across the quad. `u` runs tl→tr, `v` runs tl→bl.
    pub fn lerp(&self, u: f32, v: f32) -> Vec2 {
        let top = self.tl.lerp(self.tr, u);
        let bottom = self.bl.lerp(self.br, u);
        top.lerp(bottom, v)
    }

    /// The quad's corners divided by `size`, giving `0..1` texture coordinates.
    ///
    /// This is how a source quad measured in pixels becomes something a sampler
    /// can use, so `size` must be the size of the frame the quad was measured in.
    pub fn to_uv(&self, size: Vec2) -> Quad {
        Quad {
            tl: self.tl / size,
            tr: self.tr / size,
            br: self.br / size,
            bl: self.bl / size,
        }
    }

    /// Per-corner `q` weights for projective (perspective-correct) interpolation.
    ///
    /// # Why this is needed
    ///
    /// Drawing a quad as two triangles and interpolating texture coordinates
    /// linearly gives a *bilinear* map. For a corner-pinned slice the correct map
    /// is *projective*, and the two disagree — visibly, as a kink along the
    /// triangles' shared diagonal, because each triangle interpolates
    /// independently and the two halves only agree at the corners.
    ///
    /// The fix is the standard homogeneous-coordinate trick: give each corner a
    /// weight `q`, pass `(u*q, v*q, q)` through the rasteriser, and divide by the
    /// interpolated `q` in the fragment shader. `q` comes from where the quad's
    /// diagonals cross: for corner `i`, it is the ratio of the whole diagonal to
    /// the part of it on the far side of the crossing.
    ///
    /// An unwarped rectangle has its diagonals crossing at the centre, so every
    /// ratio is exactly 2/1 — uniform, which after the divide is the same as no
    /// correction at all. That is what makes this safe to apply unconditionally:
    /// the ordinary case is unaffected.
    ///
    /// Returns all-ones for a degenerate quad (parallel diagonals, zero area),
    /// which is the harmless affine fallback rather than a NaN.
    pub fn projective_weights(&self) -> [f32; 4] {
        // Diagonals tl→br and tr→bl.
        let a = self.tl;
        let b = self.br;
        let c = self.tr;
        let d = self.bl;

        let r = b - a;
        let s = d - c;
        let denom = r.x * s.y - r.y * s.x;
        if denom.abs() < 1e-9 {
            return [1.0; 4];
        }
        let t = ((c.x - a.x) * s.y - (c.y - a.y) * s.x) / denom;
        let cross = a + r * t;

        let d_tl = (cross - self.tl).length();
        let d_br = (cross - self.br).length();
        let d_tr = (cross - self.tr).length();
        let d_bl = (cross - self.bl).length();

        if d_tl < 1e-9 || d_br < 1e-9 || d_tr < 1e-9 || d_bl < 1e-9 {
            return [1.0; 4];
        }

        [
            (d_tl + d_br) / d_br,
            (d_tr + d_bl) / d_bl,
            (d_tl + d_br) / d_tl,
            (d_tr + d_bl) / d_tr,
        ]
    }

    pub fn translate(&self, d: Vec2) -> Quad {
        Quad {
            tl: self.tl + d,
            tr: self.tr + d,
            br: self.br + d,
            bl: self.bl + d,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_from_rect_is_axis_aligned() {
        let q = Quad::from_rect(Rect::new(10.0, 20.0, 100.0, 50.0));
        assert!(q.is_axis_aligned(0.001));
        assert_eq!(q.bounds(), Rect::new(10.0, 20.0, 100.0, 50.0));
    }

    #[test]
    fn rotated_quad_is_not_axis_aligned_and_its_bounds_are_bigger() {
        // A square rotated 45 degrees about its centre.
        let q = Quad::new(
            Vec2::new(50.0, 0.0),
            Vec2::new(100.0, 50.0),
            Vec2::new(50.0, 100.0),
            Vec2::new(0.0, 50.0),
        );
        assert!(!q.is_axis_aligned(0.001));
        // The bounding box is the full 100x100, but the quad only covers half of
        // that area. This is exactly the case where sampling the bbox is wrong.
        assert_eq!(q.bounds(), Rect::new(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn lerp_hits_the_corners() {
        let q = Quad::from_rect(Rect::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(q.lerp(0.0, 0.0), q.tl);
        assert_eq!(q.lerp(1.0, 0.0), q.tr);
        assert_eq!(q.lerp(1.0, 1.0), q.br);
        assert_eq!(q.lerp(0.0, 1.0), q.bl);
        assert_eq!(q.lerp(0.5, 0.5), Vec2::new(100.0, 50.0));
    }

    #[test]
    fn to_uv_divides_by_the_frame_it_was_measured_in() {
        let q = Quad::from_rect(Rect::new(960.0, 0.0, 960.0, 1080.0));
        let uv = q.to_uv(Vec2::new(1920.0, 1080.0));
        assert_eq!(uv.tl, Vec2::new(0.5, 0.0));
        assert_eq!(uv.br, Vec2::new(1.0, 1.0));
    }

    #[test]
    fn an_unwarped_rect_needs_no_projective_correction() {
        // Every weight equal means the divide is a no-op, so the ordinary
        // axis-aligned slice is bit-for-bit the plain affine path.
        let q = Quad::from_rect(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let w = q.projective_weights();
        for x in w {
            assert!(
                (x - 2.0).abs() < 1e-4,
                "expected uniform weights, got {w:?}"
            );
        }
    }

    #[test]
    fn a_trapezoid_weights_its_narrow_end_differently() {
        // A keystoned slice: top edge half the width of the bottom.
        let q = Quad::new(
            Vec2::new(25.0, 0.0),
            Vec2::new(75.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        );
        let w = q.projective_weights();
        // The narrow (top) corners must not share the wide corners' weight, or
        // the map is the bilinear one this exists to avoid.
        assert!(
            (w[0] - w[2]).abs() > 0.1,
            "top and bottom weights should differ for a trapezoid, got {w:?}"
        );
        assert!(
            (w[0] - w[1]).abs() < 1e-4,
            "the two top corners are symmetric"
        );
        assert!(
            (w[2] - w[3]).abs() < 1e-4,
            "the two bottom corners are symmetric"
        );
        assert!(w.iter().all(|x| x.is_finite() && *x > 0.0));
    }

    #[test]
    fn a_degenerate_quad_falls_back_to_affine_rather_than_nan() {
        // Zero-area: every corner in the same place.
        let z = Vec2::ZERO;
        assert_eq!(Quad::new(z, z, z, z).projective_weights(), [1.0; 4]);

        // Collapsed to a line — the diagonals are parallel, so there is no
        // crossing to measure from.
        let line = Quad::new(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 0.0),
        );
        assert!(line.projective_weights().iter().all(|w| w.is_finite()));
    }

    #[test]
    fn intersect_and_union() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);
        assert_eq!(a.intersect(&b), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 150.0, 150.0));
        assert_eq!(a.intersect(&Rect::new(200.0, 0.0, 10.0, 10.0)), None);
    }
}
