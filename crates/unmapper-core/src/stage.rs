//! The virtual stage: panels, where they sit, and what you look at them with.
//!
//! # Why a panel has two placements
//!
//! A [`Panel`] carries both a 2D [`Panel::layout`] and a 3D [`Panel::placement`],
//! and they are not alternatives — they are two views of one physical object. The
//! same panel appears in the emulation view at its layout rect and in the previz
//! view at its 3D transform, because it *is* the same piece of wall either way.
//!
//! Modelling it as one panel with two placements, rather than as separate 2D and
//! 3D scenes, is what stops the two views drifting apart as the operator edits.

use serde::{Deserialize, Serialize};

use crate::geom::{Rect, Vec2, Vec3};
use crate::slicemap::Size;

pub use glam::Quat;

/// Default LED pixel pitch, in millimetres, used only to give a freshly imported
/// slice map a plausible physical size. 2.6mm is a common touring wall.
///
/// This is a starting point the operator corrects, never a measurement.
pub const DEFAULT_PITCH_MM: f32 = 2.6;

/// A panel's pose in stage space: metres, Y up, -Z away from the audience.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement3d {
    /// Centre of the panel, in metres.
    pub translation: Vec3,
    /// Orientation. Identity faces +Z, i.e. towards the audience.
    pub rotation: Quat,
    /// Physical width and height in metres.
    pub size: Vec2,
}

impl Placement3d {
    /// A panel standing upright at the origin, `size` metres across.
    pub fn upright(size: Vec2) -> Self {
        Self {
            translation: Vec3::new(0.0, size.y / 2.0, 0.0),
            rotation: Quat::IDENTITY,
            size,
        }
    }

    /// The four corners in stage space, wound tl, tr, br, bl as seen face-on.
    pub fn corners(&self) -> [Vec3; 4] {
        let hx = self.size.x / 2.0;
        let hy = self.size.y / 2.0;
        [
            Vec3::new(-hx, hy, 0.0),
            Vec3::new(hx, hy, 0.0),
            Vec3::new(hx, -hy, 0.0),
            Vec3::new(-hx, -hy, 0.0),
        ]
        .map(|local| self.translation + self.rotation * local)
    }
}

/// The shape of a panel's LED surface, in panel-local metres.
///
/// # Why a panel needs a shape at all
///
/// A physical LED tile is a rigid flat rectangle, so for a long time four corners
/// were enough. But UnMapper imports **one panel per slice**, and a slice
/// routinely covers a whole run of tiles — a curved upstage wall, a wrapped
/// column, a folded corner. Those are the rigs the packed Advanced Output layout
/// hides, and showing them as flat is the thing previz is supposed to fix.
///
/// # Local space
///
/// Origin at the panel's centre, X right, Y up, Z towards the audience — the same
/// frame [`Placement3d::corners`] builds its corners in. `(u, v)` runs `0..1`
/// left to right and **top to bottom**, matching the source side.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "surface", rename_all = "kebab-case")]
pub enum Surface {
    /// A rigid flat tile. The default, and what a single physical panel is.
    #[default]
    Flat,
    /// Bent about the vertical axis through the panel's centre.
    ///
    /// `sweep_deg` is the total angle the panel subtends. Positive sweeps the
    /// two ends *away* from the audience, which is the common concave wrap;
    /// negative bulges towards them. The panel's width is preserved as arc
    /// length, so curving a wall does not silently make it narrower.
    Arc { sweep_deg: f32 },
    /// An explicit lattice of positions, row-major, `columns * rows` of them.
    ///
    /// The escape hatch for a surface no parameter describes — a fold, a stepped
    /// run, a shape someone measured. Positions are **absolute in panel-local
    /// metres**, not offsets, because they usually come from a measurement rather
    /// than from a deformation of something flat.
    Lattice {
        columns: u32,
        rows: u32,
        points: Vec<Vec3>,
    },
}

/// Degrees of arc per rendered segment. 5 degrees keeps the facets under a
/// millimetre of chord error on any panel a person can stand in front of, and a
/// 180-degree wrap still costs only 36 quads.
const ARC_DEGREES_PER_SEGMENT: f32 = 5.0;

impl Surface {
    /// A lattice, or `None` if it does not describe a grid.
    pub fn lattice(columns: u32, rows: u32, points: Vec<Vec3>) -> Option<Self> {
        if columns < 2 || rows < 2 {
            return None;
        }
        if points.len() as u64 != columns as u64 * rows as u64 {
            return None;
        }
        Some(Surface::Lattice {
            columns,
            rows,
            points,
        })
    }

    /// The flat lattice for a panel of `size` — the starting point for hand-editing
    /// a surface into shape.
    pub fn flat_lattice(size: Vec2, columns: u32, rows: u32) -> Option<Self> {
        if columns < 2 || rows < 2 {
            return None;
        }
        let mut points = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for col in 0..columns {
                let u = col as f32 / (columns - 1) as f32;
                let v = row as f32 / (rows - 1) as f32;
                points.push(Vec3::new(
                    (u - 0.5) * size.x,
                    (0.5 - v) * size.y,
                    0.0,
                ));
            }
        }
        Self::lattice(columns, rows, points)
    }

    pub fn is_flat(&self) -> bool {
        matches!(self, Surface::Flat)
    }

    /// How many cells across and down this surface needs to render smoothly.
    ///
    /// `(1, 1)` for a flat panel, which is what keeps an ordinary panel on the
    /// two-triangle path it has always taken.
    pub fn subdivisions(&self) -> (u32, u32) {
        match self {
            Surface::Flat => (1, 1),
            Surface::Arc { sweep_deg } => {
                let n = (sweep_deg.abs() / ARC_DEGREES_PER_SEGMENT).ceil();
                // A zero sweep is a flat panel written the long way round.
                ((n as u32).clamp(1, 128), 1)
            }
            Surface::Lattice { columns, rows, .. } => {
                (columns.saturating_sub(1).max(1), rows.saturating_sub(1).max(1))
            }
        }
    }

    /// The local-space point at `(u, v)` on a panel of `size` metres.
    pub fn local_point(&self, u: f32, v: f32, size: Vec2) -> Vec3 {
        let flat = Vec3::new((u - 0.5) * size.x, (0.5 - v) * size.y, 0.0);
        match self {
            Surface::Flat => flat,
            Surface::Arc { sweep_deg } => {
                let theta = sweep_deg.to_radians();
                // Straight in the limit: the radius goes to infinity, and dividing
                // by a near-zero angle would throw the panel to the horizon.
                if theta.abs() < 1e-6 {
                    return flat;
                }
                // Width is arc length, so the chord comes out shorter than a flat
                // panel of the same pixel count — which is what really happens
                // when you curve a wall.
                let radius = size.x / theta;
                let a = (u - 0.5) * theta;
                Vec3::new(
                    radius * a.sin(),
                    (0.5 - v) * size.y,
                    radius * (a.cos() - 1.0),
                )
            }
            Surface::Lattice {
                columns,
                rows,
                points,
            } => {
                let (cols, rws) = (*columns, *rows);
                if cols < 2 || rws < 2 || points.len() as u64 != cols as u64 * rws as u64 {
                    return flat;
                }
                // Which cell, and where inside it. Bilinear within a cell, which
                // is the straight-edged reading — the same one the warp lattice
                // gets, and for the same reason: anything else is a guess about
                // what happens between measured points.
                let (c0, r0, fu, fv) = crate::geom::lattice_cell(u, v, cols, rws);
                let at = |c: u32, r: u32| points[(r * cols + c) as usize];
                let top = at(c0, r0).lerp(at(c0 + 1, r0), fu);
                let bottom = at(c0, r0 + 1).lerp(at(c0 + 1, r0 + 1), fu);
                top.lerp(bottom, fv)
            }
        }
    }
}

/// One physical LED surface in the virtual stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub name: String,
    /// The panel's own LED resolution. This is what makes emulation pixel-exact:
    /// one texel of the panel is one LED.
    pub pixels: Size,
    /// Where it sits on the emulation canvas, in virtual raster pixels.
    pub layout: Rect,
    /// Where it sits in the physical stage, in metres.
    pub placement: Placement3d,
    /// The shape of its LED surface. [`Surface::Flat`] unless someone has said
    /// otherwise, so an imported rig behaves exactly as it did before surfaces
    /// existed.
    #[serde(default)]
    pub surface: Surface,
    pub enabled: bool,
}

impl Panel {
    /// A panel of `pixels` resolution at `layout`, given a physical size implied
    /// by `pitch_mm`.
    pub fn from_layout(
        id: impl Into<String>,
        name: impl Into<String>,
        pixels: Size,
        layout: Rect,
        pitch_mm: f32,
    ) -> Self {
        let size_m = Vec2::new(
            pixels.width as f32 * pitch_mm / 1000.0,
            pixels.height as f32 * pitch_mm / 1000.0,
        );
        Self {
            id: id.into(),
            name: name.into(),
            pixels,
            layout,
            placement: Placement3d::upright(size_m),
            surface: Surface::Flat,
            enabled: true,
        }
    }

    /// The stage-space point at `(u, v)` across this panel's LED surface.
    ///
    /// This is what the previz renderer walks to build a panel's geometry, and
    /// the only place the surface shape and the panel's pose come together. For a
    /// flat panel it agrees exactly with [`Placement3d::corners`] at the corners.
    pub fn surface_point(&self, u: f32, v: f32) -> Vec3 {
        self.placement.translation
            + self.placement.rotation * self.surface.local_point(u, v, self.placement.size)
    }

    /// How many cells across and down this panel's surface needs.
    pub fn subdivisions(&self) -> (u32, u32) {
        self.surface.subdivisions()
    }

    /// Pixel pitch in millimetres implied by the current physical size.
    ///
    /// Returns `None` for a zero-width panel rather than an infinity.
    pub fn pitch_mm(&self) -> Option<f32> {
        if self.pixels.width == 0 {
            return None;
        }
        Some(self.placement.size.x * 1000.0 / self.pixels.width as f32)
    }
}

/// A flat image standing in for the display surface — a render, a plan, a photo
/// of the set. Positioned on the emulation canvas so panels can be dragged onto
/// the places they occupy in the mockup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backdrop {
    pub path: std::path::PathBuf,
    /// Where the image sits on the emulation canvas, in virtual raster pixels.
    pub rect: Rect,
    /// 0..1, so panels stay readable over a busy render.
    pub opacity: f32,
}

/// A 3D model of the set, for the previz view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model3d {
    pub path: std::path::PathBuf,
    /// Applied to the model on load, for the usual unit and axis disagreements.
    pub scale: f32,
    pub rotation: Quat,
    pub translation: Vec3,
}

impl Default for Model3d {
    fn default() -> Self {
        Self {
            path: std::path::PathBuf::new(),
            scale: 1.0,
            rotation: Quat::IDENTITY,
            translation: Vec3::ZERO,
        }
    }
}

/// Both geometry backings. Either, neither or both may be present — a 2D mockup
/// and a CAD model are different ways to describe the same set, and an operator
/// with both should not have to choose.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StageGeometry {
    pub backdrop: Option<Backdrop>,
    pub model: Option<Model3d>,
}

/// A previz camera.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        // Roughly front-of-house: 12m back, eye height, looking at the middle of
        // a wall 4m up.
        Self {
            position: Vec3::new(0.0, 1.7, 12.0),
            target: Vec3::new(0.0, 4.0, 0.0),
            up: Vec3::Y,
            fov_y_deg: 39.6, // a 50mm lens on full frame
            near: 0.05,
            far: 500.0,
        }
    }
}

impl Camera {
    pub fn view_matrix(&self) -> glam::Mat4 {
        glam::Mat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn projection_matrix(&self, aspect: f32) -> glam::Mat4 {
        glam::Mat4::perspective_rh(self.fov_y_deg.to_radians(), aspect, self.near, self.far)
    }

    pub fn view_projection(&self, aspect: f32) -> glam::Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_survives_the_round_trip() {
        let p = Panel::from_layout(
            "p",
            "Panel",
            Size::new(1920, 1080),
            Rect::from_size(1920.0, 1080.0),
            2.6,
        );
        // 1920 px at 2.6mm is just under 5 metres wide.
        assert!((p.placement.size.x - 4.992).abs() < 1e-4);
        assert!((p.pitch_mm().unwrap() - 2.6).abs() < 1e-4);
    }

    #[test]
    fn zero_width_panel_has_no_pitch_rather_than_an_infinity() {
        let mut p = Panel::from_layout("p", "P", Size::new(0, 0), Rect::from_size(0.0, 0.0), 2.6);
        p.pixels = Size::new(0, 1);
        assert_eq!(p.pitch_mm(), None);
    }

    #[test]
    fn upright_panel_sits_on_the_deck() {
        let pl = Placement3d::upright(Vec2::new(4.0, 3.0));
        let corners = pl.corners();
        // Bottom edge at y=0, top at y=3.
        let lowest = corners.iter().map(|c| c.y).fold(f32::INFINITY, f32::min);
        let highest = corners
            .iter()
            .map(|c| c.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(lowest.abs() < 1e-5, "bottom edge should be on the deck");
        assert!((highest - 3.0).abs() < 1e-5);
    }

    #[test]
    fn rotated_panel_corners_follow_the_rotation() {
        let mut pl = Placement3d::upright(Vec2::new(4.0, 2.0));
        pl.translation = Vec3::ZERO;
        pl.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        // Turned 90 degrees about Y, a 4m-wide panel now spans 4m in Z and none in X.
        let c = pl.corners();
        let span_x = c.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max)
            - c.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let span_z = c.iter().map(|p| p.z).fold(f32::NEG_INFINITY, f32::max)
            - c.iter().map(|p| p.z).fold(f32::INFINITY, f32::min);
        assert!(span_x < 1e-5, "x span should collapse, got {span_x}");
        assert!(
            (span_z - 4.0).abs() < 1e-5,
            "z span should be 4, got {span_z}"
        );
    }

    #[test]
    fn camera_projects_its_target_to_the_centre_of_the_frame() {
        let cam = Camera::default();
        let vp = cam.view_projection(16.0 / 9.0);
        let clip = vp * cam.target.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(
            ndc.x.abs() < 1e-5,
            "target should be centred in x, got {}",
            ndc.x
        );
        assert!(
            ndc.y.abs() < 1e-5,
            "target should be centred in y, got {}",
            ndc.y
        );
    }

    #[test]
    fn a_flat_surface_agrees_with_the_four_corners() {
        // The compatibility guarantee: every rig in existence is flat, and the
        // surface path must not move any of them by a millimetre.
        let panel = Panel::from_layout("p", "P", Size::new(400, 200), Rect::new(0.0, 0.0, 400.0, 200.0), 2.6);
        let corners = panel.placement.corners();
        let at = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for (i, (u, v)) in at.iter().enumerate() {
            let p = panel.surface_point(*u, *v);
            assert!(
                (p - corners[i]).length() < 1e-5,
                "corner {i}: surface gave {p:?}, corners() gave {:?}",
                corners[i]
            );
        }
        assert_eq!(panel.subdivisions(), (1, 1));
    }

    #[test]
    fn an_arc_keeps_its_width_as_arc_length_and_sweeps_its_ends_away() {
        let mut panel =
            Panel::from_layout("p", "P", Size::new(1000, 200), Rect::new(0.0, 0.0, 1000.0, 200.0), 2.6);
        panel.placement.translation = Vec3::ZERO;
        panel.placement.rotation = Quat::IDENTITY;
        let width = panel.placement.size.x;
        panel.surface = Surface::Arc { sweep_deg: 90.0 };

        // Walk the surface and add up the chords: a curved wall must not lose
        // pixels' worth of width just because it was bent.
        let steps = 512;
        let mut length = 0.0;
        let mut prev = panel.surface_point(0.0, 0.5);
        for i in 1..=steps {
            let p = panel.surface_point(i as f32 / steps as f32, 0.5);
            length += (p - prev).length();
            prev = p;
        }
        assert!(
            (length - width).abs() < width * 1e-3,
            "arc length {length} should match the flat width {width}"
        );

        // The centre stays on the panel's plane and both ends recede from the
        // audience. A positive sweep that bulged forwards would be the sign flip
        // that makes every curved rig inside out.
        assert!(panel.surface_point(0.5, 0.5).z.abs() < 1e-4);
        assert!(panel.surface_point(0.0, 0.5).z < -0.01);
        assert!(panel.surface_point(1.0, 0.5).z < -0.01);
        assert!((panel.surface_point(0.0, 0.5).z - panel.surface_point(1.0, 0.5).z).abs() < 1e-4);
        assert!(Surface::Arc { sweep_deg: -90.0 }.local_point(0.0, 0.5, panel.placement.size).z > 0.01);

        // 90 degrees at 5 per segment.
        assert_eq!(panel.subdivisions(), (18, 1));
    }

    #[test]
    fn a_zero_sweep_arc_is_flat_rather_than_a_division_by_zero() {
        let size = Vec2::new(4.0, 2.0);
        for sweep in [0.0, 1e-9, -1e-9] {
            let p = Surface::Arc { sweep_deg: sweep }.local_point(0.25, 0.75, size);
            let flat = Surface::Flat.local_point(0.25, 0.75, size);
            assert!((p - flat).length() < 1e-6, "sweep {sweep} gave {p:?}");
            assert!(p.is_finite(), "sweep {sweep} produced {p:?}");
        }
    }

    #[test]
    fn a_flat_lattice_is_the_flat_panel_and_a_pulled_point_moves_only_near_itself() {
        let size = Vec2::new(4.0, 2.0);
        let flat = Surface::flat_lattice(size, 3, 3).expect("a 3x3 lattice");
        for (u, v) in [(0.0, 0.0), (0.5, 0.5), (0.25, 0.9), (1.0, 1.0)] {
            let a = flat.local_point(u, v, size);
            let b = Surface::Flat.local_point(u, v, size);
            assert!((a - b).length() < 1e-5, "({u},{v}): {a:?} vs {b:?}");
        }

        // Pull the centre point towards the audience.
        let Surface::Lattice { columns, rows, mut points } = flat else {
            panic!("flat_lattice should build a lattice");
        };
        points[4].z += 1.0;
        let pulled = Surface::lattice(columns, rows, points).unwrap();
        assert!((pulled.local_point(0.5, 0.5, size).z - 1.0).abs() < 1e-5);
        // The corners are pinned by their own control points.
        assert!(pulled.local_point(0.0, 0.0, size).z.abs() < 1e-5);
        assert_eq!(pulled.subdivisions(), (2, 2));
    }

    #[test]
    fn a_lattice_that_is_not_a_grid_is_refused_and_never_indexes_out_of_range() {
        assert!(Surface::lattice(1, 3, vec![Vec3::ZERO; 3]).is_none());
        assert!(Surface::lattice(3, 3, vec![Vec3::ZERO; 8]).is_none());
        assert!(Surface::flat_lattice(Vec2::new(1.0, 1.0), 4, 1).is_none());

        // A hand-edited stage file can still carry a broken lattice. It must fall
        // back to flat, not panic once per frame inside the renderer.
        let broken = Surface::Lattice { columns: 4, rows: 4, points: vec![Vec3::ZERO; 3] };
        let size = Vec2::new(4.0, 2.0);
        for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
            assert_eq!(broken.local_point(u, v, size), Surface::Flat.local_point(u, v, size));
        }
    }

    #[test]
    fn surface_uv_edges_stay_inside_the_lattice() {
        // v = 1.0 exactly lands on the last row's boundary; an off-by-one in the
        // cell search panics there and nowhere else.
        let size = Vec2::new(4.0, 2.0);
        let lattice = Surface::flat_lattice(size, 4, 4).unwrap();
        for (u, v) in [(1.0, 1.0), (0.0, 1.0), (1.0, 0.0), (-0.5, 1.5)] {
            assert!(lattice.local_point(u, v, size).is_finite());
        }
    }
}
