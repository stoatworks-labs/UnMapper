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

/// Locate `(u, v)` in a `columns` x `rows` lattice of control points.
///
/// Returns the top-left index of the containing cell and the fraction within it,
/// so a caller can interpolate between four neighbours. Used by both lattices in
/// the model — the warp mesh in source space and the panel surface in stage space
/// — because they index identically and only differ in what they hold.
///
/// The clamp is on the **cell index**, never on the scaled coordinate. Clamping
/// before the floor discards the fraction and collapses the last row and column
/// onto their leading edge, which reads as a panel that is subtly squashed at two
/// of its four sides.
pub fn lattice_cell(u: f32, v: f32, columns: u32, rows: u32) -> (u32, u32, f32, f32) {
    let su = u.clamp(0.0, 1.0) * (columns.saturating_sub(1)) as f32;
    let sv = v.clamp(0.0, 1.0) * (rows.saturating_sub(1)) as f32;
    let c0 = (su.floor() as u32).min(columns.saturating_sub(2));
    let r0 = (sv.floor() as u32).min(rows.saturating_sub(2));
    (c0, r0, su - c0 as f32, sv - r0 as f32)
}

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

    /// The point at `(u, v)` under the quad's **projective** map, `u` running
    /// tl→tr and `v` running tl→bl.
    ///
    /// [`Quad::lerp`] is the bilinear map and disagrees with this one everywhere
    /// except the four corners — the same disagreement documented on
    /// [`Quad::projective_weights`], and the reason that function exists.
    ///
    /// # Why this matters for subdivision
    ///
    /// Cutting a quad into cells and giving each cell its own projective weights
    /// reproduces the original map *exactly*, but only if the cell corners
    /// themselves came from the projective map. A homography restricted to a
    /// sub-rectangle of the unit square is still a homography, and a homography is
    /// pinned by four point correspondences — so the sub-quad's own weights
    /// rebuild it. Take the corners from `lerp` instead and every cell is subtly
    /// misplaced, which is the bilinear bug reintroduced through the back door.
    ///
    /// A parallelogram has no perspective in it, so this reduces to `lerp`.
    ///
    /// # Not the same thing as `projective_weights`
    ///
    /// [`Quad::projective_weights`] is valid in the *other* direction — mapping
    /// the quad's corners to texture coordinates that a rasteriser interpolates,
    /// which is what the shader does. Reusing those weights to compute a position
    /// from `(u, v)` gives a map whose numerator keeps a `uv` term, so it is not
    /// projective at all and misses the true point by whole pixels on a keystone.
    /// This solves the actual homography instead (Heckbert's closed form for the
    /// unit square, with `Quad`'s winding as its corner order).
    pub fn project(&self, u: f32, v: f32) -> Vec2 {
        let (p0, p1, p2, p3) = (self.tl, self.tr, self.br, self.bl);

        let dx1 = p1.x - p2.x;
        let dx2 = p3.x - p2.x;
        let sx = p0.x - p1.x + p2.x - p3.x;
        let dy1 = p1.y - p2.y;
        let dy2 = p3.y - p2.y;
        let sy = p0.y - p1.y + p2.y - p3.y;

        let den = dx1 * dy2 - dx2 * dy1;

        // No perspective term, or a degenerate quad with no usable solve: the map
        // is affine and bilinear is exact.
        if (sx.abs() < 1e-9 && sy.abs() < 1e-9) || den.abs() < 1e-12 {
            return self.lerp(u, v);
        }

        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;

        let w = g * u + h * v + 1.0;
        if w.abs() < 1e-9 {
            // On the vanishing line. Nothing sensible to return; the affine
            // fallback at least stays finite rather than exploding to NaN.
            return self.lerp(u, v);
        }

        let a = (p1.x - p0.x) + g * p1.x;
        let b = (p3.x - p0.x) + h * p3.x;
        let d = (p1.y - p0.y) + g * p1.y;
        let e = (p3.y - p0.y) + h * p3.y;

        Vec2::new((a * u + b * v + p0.x) / w, (d * u + e * v + p0.y) / w)
    }

    /// The sub-quad covering `[u0,u1] x [v0,v1]` of this one, projectively.
    ///
    /// Use this rather than four `project` calls when building a grid, so the
    /// corner order cannot drift from [`Quad`]'s own winding.
    pub fn sub_quad(&self, u0: f32, v0: f32, u1: f32, v1: f32) -> Quad {
        Quad::new(
            self.project(u0, v0),
            self.project(u1, v0),
            self.project(u1, v1),
            self.project(u0, v1),
        )
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

/// A ray, in whatever space the holder says.
///
/// Exists so a click in the previz view can become a point in the stage: the
/// camera turns a screen position into one of these, and the thing being dragged
/// says what surface to meet it on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// Unit length by construction in [`Ray::new`], so `t` is metres.
    pub direction: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalize_or_zero(),
        }
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }

    /// Where this ray meets the plane through `point` with `normal`.
    ///
    /// `None` when the ray runs along the plane, or meets it *behind* the origin
    /// — dragging a handle must never teleport it to a mirrored point somewhere
    /// behind the camera, which is what an unsigned intersection does the moment
    /// the pointer crosses the horizon.
    pub fn intersect_plane(&self, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let denom = normal.dot(self.direction);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = normal.dot(point - self.origin) / denom;
        if !(t.is_finite() && t > 0.0) {
            return None;
        }
        Some(self.at(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_meets_a_plane_in_front_of_it_and_never_one_behind() {
        let r = Ray::new(Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = r
            .intersect_plane(Vec3::ZERO, Vec3::Z)
            .expect("meets the plane");
        assert!((hit - Vec3::ZERO).length() < 1e-5, "got {hit:?}");

        // The same plane, now behind the ray: no hit, rather than one at -t.
        let away = Ray::new(Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(away.intersect_plane(Vec3::ZERO, Vec3::Z), None);

        // And a ray running along the plane misses it rather than dividing by zero.
        let along = Ray::new(Vec3::new(0.0, 0.0, 10.0), Vec3::X);
        assert_eq!(along.intersect_plane(Vec3::ZERO, Vec3::Z), None);
    }

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
    fn project_hits_the_corners_and_reduces_to_lerp_on_a_rectangle() {
        let keystone = Quad::new(
            Vec2::new(25.0, 0.0),
            Vec2::new(75.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        );
        assert!((keystone.project(0.0, 0.0) - keystone.tl).length() < 1e-3);
        assert!((keystone.project(1.0, 0.0) - keystone.tr).length() < 1e-3);
        assert!((keystone.project(1.0, 1.0) - keystone.br).length() < 1e-3);
        assert!((keystone.project(0.0, 1.0) - keystone.bl).length() < 1e-3);

        // Checked against a homography solved independently by least squares:
        // the centre of the unit square lands at (50, 33.333), not at the
        // bilinear (50, 50). Getting this backwards is a whole-pixel error.
        let mid = keystone.project(0.5, 0.5);
        assert!(
            (mid - Vec2::new(50.0, 100.0 / 3.0)).length() < 1e-3,
            "expected the true homography's centre, got {mid:?}"
        );
        assert!((keystone.project(0.25, 0.75) - Vec2::new(30.0, 60.0)).length() < 1e-3);
        assert!(
            (mid - keystone.lerp(0.5, 0.5)).length() > 1.0,
            "projective and bilinear must differ, or this is doing nothing"
        );

        // On a plain rectangle the weights are uniform and the two agree.
        let rect = Quad::from_rect(Rect::new(10.0, 20.0, 100.0, 50.0));
        for (u, v) in [(0.25, 0.75), (0.5, 0.5), (0.9, 0.1)] {
            assert!((rect.project(u, v) - rect.lerp(u, v)).length() < 1e-3);
        }
    }

    #[test]
    fn subdividing_projectively_reproduces_the_whole_map() {
        // The property the renderer's grid depends on: split a corner-pinned quad
        // into cells, take each cell's own projective weights, and the map is
        // unchanged. If this fails, a curved panel resamples its source wrongly.
        let keystone = Quad::new(
            Vec2::new(25.0, 0.0),
            Vec2::new(75.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        );

        let n = 4;
        for row in 0..n {
            for col in 0..n {
                let (u0, u1) = (col as f32 / n as f32, (col + 1) as f32 / n as f32);
                let (v0, v1) = (row as f32 / n as f32, (row + 1) as f32 / n as f32);
                let cell = keystone.sub_quad(u0, v0, u1, v1);

                // A point in the middle of the cell, found two ways.
                for (su, sv) in [(0.5, 0.5), (0.25, 0.8)] {
                    let via_cell = cell.project(su, sv);
                    let via_whole = keystone.project(u0 + (u1 - u0) * su, v0 + (v1 - v0) * sv);
                    assert!(
                        (via_cell - via_whole).length() < 1e-2,
                        "cell ({col},{row}) at ({su},{sv}): {via_cell:?} vs {via_whole:?}"
                    );
                }
            }
        }
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
