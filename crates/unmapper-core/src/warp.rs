//! Warp meshes — a slice whose content is not a plain rectangle.
//!
//! # What a warp mesh is
//!
//! Resolume gives every slice a `Warper`: a `columns` x `rows` lattice of control
//! points saying where the slice's content actually lands, instead of the four
//! corners of its output rect. An operator drags those points when the surface the
//! slice feeds is not a flat rectangle — a curved wall, a folded corner, a run of
//! panels that steps.
//!
//! The points are in **the same space as the quad they belong to**, which for an
//! imported slice is its screen's raster space. They are stored **row-major**:
//! index `row * columns + col`, `col` varying fastest. Both of those were read off
//! real Arena 7.27.0 files, where an untouched warper is an evenly spaced lattice
//! spanning the slice's output rect exactly — see the fixtures in
//! `unmapper-resolume`.
//!
//! # Why the identity case is kept separate
//!
//! Arena writes a full warper for *every* slice whether or not anyone has touched
//! it, so most meshes are the identity. [`WarpMesh::is_identity`] exists so those
//! can take the ordinary single-quad render path rather than being subdivided into
//! a grid that computes the same picture more slowly — and, more importantly, so
//! the ordinary case stays bit-for-bit what it was before warping existed.
//!
//! # The interpolation trap
//!
//! Subdividing a quad and interpolating across the cells linearly is a *bilinear*
//! map. For a corner-pinned quad the correct map is *projective*, and the two
//! disagree away from the corners — the same trap documented on
//! [`crate::geom::Quad::projective_weights`]. So a cell is never assumed to be a
//! plain lerp of the whole: each cell carries its own corners and gets its own
//! projective weights at draw time.

use serde::{Deserialize, Serialize};

use crate::geom::{Quad, Vec2};

/// How Resolume interpolates between the control points.
///
/// Only [`WarpMode::Linear`] is reproduced. Arena's `Point Mode` param is written
/// as `PM_LINEAR` on every real file available here; any other value is carried
/// through as [`WarpMode::Other`] so it survives a round trip and can be reported,
/// but it is **not** guessed at — a curve interpolated the wrong way is a wall
/// that is wrong in a new and less obvious way than one that is merely flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum WarpMode {
    /// `PM_LINEAR` — straight edges between adjacent control points.
    Linear,
    /// Anything else Resolume wrote, kept verbatim.
    Other { name: String },
}

impl WarpMode {
    /// Resolume's own name for this mode.
    pub fn as_str(&self) -> &str {
        match self {
            WarpMode::Linear => "PM_LINEAR",
            WarpMode::Other { name } => name,
        }
    }

    /// Read a `Point Mode` param value.
    pub fn from_param(value: &str) -> Self {
        match value.trim() {
            "PM_LINEAR" | "" => WarpMode::Linear,
            other => WarpMode::Other {
                name: other.to_owned(),
            },
        }
    }

    /// Whether UnMapper can actually reproduce this mode.
    pub fn is_reproducible(&self) -> bool {
        matches!(self, WarpMode::Linear)
    }
}

/// One cell of a subdivided mesh: where it reads from, and where it belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpCell {
    /// The cell's four corners in the mesh's own space — raster pixels for an
    /// imported slice. Feeds a sampler, so it stays a quad.
    pub source: Quad,
    /// The cell's footprint across the whole panel, `0..1` on both axes. The
    /// renderer maps this onto whatever destination the panel has, flat or 3D.
    pub dest_uv: Quad,
}

/// A lattice of control points deforming a slice.
///
/// Invariant: `points.len() == columns * rows`, with `columns >= 2` and
/// `rows >= 2`. Construct through [`WarpMesh::new`], which enforces it; everything
/// that reads the lattice returns `Option` rather than indexing blind, because a
/// malformed mesh reaching the renderer would panic once per frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarpMesh {
    pub columns: u32,
    pub rows: u32,
    /// Row-major: index `row * columns + col`.
    pub points: Vec<Vec2>,
    pub mode: WarpMode,
}

impl WarpMesh {
    /// A mesh, or `None` if the lattice does not describe a grid.
    ///
    /// A 1xN or Nx1 "grid" is rejected along with a mis-sized point list: a mesh
    /// with no interior has no cells, so it can only ever be the four corners the
    /// caller already had.
    pub fn new(columns: u32, rows: u32, points: Vec<Vec2>, mode: WarpMode) -> Option<Self> {
        if columns < 2 || rows < 2 {
            return None;
        }
        if points.len() as u64 != columns as u64 * rows as u64 {
            return None;
        }
        Some(Self {
            columns,
            rows,
            points,
            mode,
        })
    }

    /// The untouched lattice spanning `quad` — what Arena writes for a slice
    /// nobody has warped.
    pub fn identity(quad: Quad, columns: u32, rows: u32) -> Option<Self> {
        if columns < 2 || rows < 2 {
            return None;
        }
        let mut points = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for col in 0..columns {
                let u = col as f32 / (columns - 1) as f32;
                let v = row as f32 / (rows - 1) as f32;
                points.push(quad.lerp(u, v));
            }
        }
        Self::new(columns, rows, points, WarpMode::Linear)
    }

    /// The control point at `(col, row)`, or `None` if it is off the lattice.
    pub fn point(&self, col: u32, row: u32) -> Option<Vec2> {
        if col >= self.columns || row >= self.rows {
            return None;
        }
        self.points.get((row * self.columns + col) as usize).copied()
    }

    /// Cells across, and cells down.
    pub fn cell_counts(&self) -> (u32, u32) {
        (self.columns.saturating_sub(1), self.rows.saturating_sub(1))
    }

    /// Whether this lattice is the untouched one for `quad`, within `eps` pixels.
    ///
    /// This is the test for whether the slice can take the cheap single-quad path.
    /// It compares every point, not just the corners: dragging an interior point
    /// leaves all four corners where they were, which is exactly the common case
    /// an outline- or homography-based check misses.
    pub fn is_identity(&self, quad: Quad, eps: f32) -> bool {
        let Some(ident) = Self::identity(quad, self.columns, self.rows) else {
            return true;
        };
        self.points
            .iter()
            .zip(ident.points.iter())
            .all(|(a, b)| (*a - *b).length() <= eps)
    }

    /// The largest distance any control point sits from where an untouched
    /// lattice would put it, in the mesh's own units.
    ///
    /// Reported to the operator so "this slice is warped" comes with a sense of
    /// *how much* — a 0.6 px rounding artefact and a 200 px fold both trip
    /// [`WarpMesh::is_identity`], and they are not the same news.
    pub fn max_deviation(&self, quad: Quad) -> f32 {
        let Some(ident) = Self::identity(quad, self.columns, self.rows) else {
            return 0.0;
        };
        self.points
            .iter()
            .zip(ident.points.iter())
            .map(|(a, b)| (*a - *b).length())
            .fold(0.0, f32::max)
    }

    /// Every cell, left to right then top to bottom.
    ///
    /// `dest_uv` is the cell's share of the panel and `source` is where to read
    /// it from. An identity mesh yields cells whose two quads agree, which is why
    /// drawing one is the same picture as drawing the single quad — just in more
    /// triangles.
    pub fn cells(&self) -> impl Iterator<Item = WarpCell> + '_ {
        let (cols, rows) = self.cell_counts();
        (0..rows).flat_map(move |row| {
            (0..cols).filter_map(move |col| {
                let source = Quad::new(
                    self.point(col, row)?,
                    self.point(col + 1, row)?,
                    self.point(col + 1, row + 1)?,
                    self.point(col, row + 1)?,
                );
                let u0 = col as f32 / cols as f32;
                let u1 = (col + 1) as f32 / cols as f32;
                let v0 = row as f32 / rows as f32;
                let v1 = (row + 1) as f32 / rows as f32;
                let dest_uv = Quad::new(
                    Vec2::new(u0, v0),
                    Vec2::new(u1, v0),
                    Vec2::new(u1, v1),
                    Vec2::new(u0, v1),
                );
                Some(WarpCell { source, dest_uv })
            })
        })
    }

    /// The source position at `(u, v)` across the whole lattice.
    ///
    /// `PM_LINEAR` means straight edges between adjacent control points, so this
    /// is bilinear *within a cell* — not across the lattice, which would smooth
    /// away the creases the operator put there deliberately.
    ///
    /// Needed because the render grid is not always the lattice's own grid: a
    /// curved panel subdivides far more finely than a 4x4 warper, and both have
    /// to be evaluated on one shared grid.
    pub fn source_at(&self, u: f32, v: f32) -> Vec2 {
        let (c0, r0, fu, fv) = crate::geom::lattice_cell(u, v, self.columns, self.rows);
        let at = |c: u32, r: u32| self.point(c, r).unwrap_or(Vec2::ZERO);
        let top = at(c0, r0).lerp(at(c0 + 1, r0), fu);
        let bottom = at(c0, r0 + 1).lerp(at(c0 + 1, r0 + 1), fu);
        top.lerp(bottom, fv)
    }

    /// Move the whole lattice, for when its quad moves.
    pub fn translate(&self, d: Vec2) -> Self {
        Self {
            columns: self.columns,
            rows: self.rows,
            points: self.points.iter().map(|p| *p + d).collect(),
            mode: self.mode.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    fn quad() -> Quad {
        Quad::from_rect(Rect::new(0.0, 0.0, 1024.0, 640.0))
    }

    #[test]
    fn an_identity_mesh_matches_the_lattice_arena_writes() {
        // The real numbers from `resolume-arena-preset.xml`: a 4x4 lattice over a
        // 1024x640 slice at the origin. If this drifts, the reader is no longer
        // reading what Arena writes.
        let mesh = WarpMesh::identity(quad(), 4, 4).expect("a 4x4 grid");
        assert_eq!(mesh.points.len(), 16);
        assert_eq!(mesh.point(0, 0), Some(Vec2::new(0.0, 0.0)));
        assert_eq!(mesh.point(3, 3), Some(Vec2::new(1024.0, 640.0)));
        // Row-major: index 1 is one step across, not one step down.
        let across = mesh.point(1, 0).unwrap();
        assert!((across.x - 1024.0 / 3.0).abs() < 1e-3, "got {across:?}");
        assert!(across.y.abs() < 1e-3, "got {across:?}");
        let down = mesh.point(0, 1).unwrap();
        assert!((down.y - 640.0 / 3.0).abs() < 1e-3, "got {down:?}");
    }

    #[test]
    fn an_untouched_mesh_is_identity_and_a_dragged_interior_point_is_not() {
        let mut mesh = WarpMesh::identity(quad(), 4, 4).unwrap();
        assert!(mesh.is_identity(quad(), 0.5));
        assert_eq!(mesh.max_deviation(quad()), 0.0);

        // Drag one interior point. Every corner stays exactly where it was, which
        // is what makes this the case a corner- or homography-based check misses.
        mesh.points[5] += Vec2::new(0.0, 40.0);
        assert!(!mesh.is_identity(quad(), 0.5));
        assert!((mesh.max_deviation(quad()) - 40.0).abs() < 1e-3);
        let corners = [mesh.point(0, 0), mesh.point(3, 0), mesh.point(3, 3)];
        let ident = WarpMesh::identity(quad(), 4, 4).unwrap();
        assert_eq!(
            corners,
            [ident.point(0, 0), ident.point(3, 0), ident.point(3, 3)]
        );
    }

    #[test]
    fn cells_tile_the_panel_exactly_once() {
        let mesh = WarpMesh::identity(quad(), 4, 4).unwrap();
        let cells: Vec<_> = mesh.cells().collect();
        assert_eq!(cells.len(), 9, "a 4x4 lattice is a 3x3 grid of cells");

        // The destination footprints must cover 0..1 with no gap and no overlap,
        // or the panel shows seams.
        assert_eq!(cells[0].dest_uv.tl, Vec2::new(0.0, 0.0));
        assert_eq!(cells[8].dest_uv.br, Vec2::new(1.0, 1.0));
        assert_eq!(cells[0].dest_uv.tr, cells[1].dest_uv.tl);
        assert_eq!(cells[0].dest_uv.bl, cells[3].dest_uv.tl);

        // For an identity mesh the source of each cell is the same region of the
        // quad as its destination, scaled up — that is what makes subdividing an
        // untouched slice a no-op.
        let first = cells[0];
        assert_eq!(first.source.tl, Vec2::new(0.0, 0.0));
        assert!((first.source.br.x - 1024.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn a_lattice_with_no_interior_is_rejected() {
        // 1xN has no cells, so it can only restate the corners.
        assert!(WarpMesh::new(1, 4, vec![Vec2::ZERO; 4], WarpMode::Linear).is_none());
        assert!(WarpMesh::identity(quad(), 4, 1).is_none());
        // A mis-sized point list must not become a mesh that panics when indexed.
        assert!(WarpMesh::new(4, 4, vec![Vec2::ZERO; 15], WarpMode::Linear).is_none());
    }

    #[test]
    fn only_linear_is_claimed_as_reproducible() {
        assert_eq!(WarpMode::from_param("PM_LINEAR"), WarpMode::Linear);
        assert!(WarpMode::from_param("PM_LINEAR").is_reproducible());
        let other = WarpMode::from_param("PM_BEZIER");
        assert!(!other.is_reproducible());
        // Carried verbatim, so a round trip does not quietly rewrite the file.
        assert_eq!(other.as_str(), "PM_BEZIER");
    }
}
