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

use crate::geom::{Ray, Rect, Vec2, Vec3};
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

/// What an arc actually measures, once it is bent.
///
/// The operator types a sweep because that is what the shape *is*, but the
/// numbers they can check against a drawing or a tape measure are these — a
/// radius to compare with the truss circle, the straight-line span between the
/// two ends, and how far the middle stands proud of that line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcMetrics {
    /// Radius of the circle the panel lies on, in metres.
    pub radius: f32,
    /// Straight-line distance between the two ends, in metres. Always shorter
    /// than the panel's width, which is preserved as arc length.
    pub chord: f32,
    /// How far the centre of the panel sits from that chord, in metres.
    pub depth: f32,
}

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
                points.push(Vec3::new((u - 0.5) * size.x, (0.5 - v) * size.y, 0.0));
            }
        }
        Self::lattice(columns, rows, points)
    }

    /// Resample this surface onto a `columns` x `rows` lattice of the same shape.
    ///
    /// This is how the editor gets from a shape a parameter describes to one it
    /// can drag: bake the arc, then pull the points. Sampling — rather than
    /// converting the parameters — is what keeps it honest, because the lattice
    /// is built from the very function the renderer walks, so the picture does
    /// not move on the frame the conversion happens.
    ///
    /// Re-baking a lattice at its own size is the identity: the samples land
    /// exactly on the existing control points. That matters, because the columns
    /// and rows spinners re-bake on every change, and a resample that drifted
    /// would erode a measured surface a nudge at a time.
    pub fn bake_lattice(&self, size: Vec2, columns: u32, rows: u32) -> Option<Self> {
        if columns < 2 || rows < 2 {
            return None;
        }
        let mut points = Vec::with_capacity((columns as usize) * (rows as usize));
        for row in 0..rows {
            for col in 0..columns {
                let u = col as f32 / (columns - 1) as f32;
                let v = row as f32 / (rows - 1) as f32;
                points.push(self.local_point(u, v, size));
            }
        }
        Self::lattice(columns, rows, points)
    }

    /// The lattice's size, or `None` for a surface that is not one.
    pub fn lattice_dims(&self) -> Option<(u32, u32)> {
        match self {
            Surface::Lattice { columns, rows, .. } => Some((*columns, *rows)),
            _ => None,
        }
    }

    /// The control points, or an empty slice for a surface that has none.
    pub fn points(&self) -> &[Vec3] {
        match self {
            Surface::Lattice { points, .. } => points,
            _ => &[],
        }
    }

    /// Move one control point, in panel-local metres. `false` if there is no such
    /// point — a stale index from a surface that changed under the selection.
    pub fn set_point(&mut self, index: usize, to: Vec3) -> bool {
        match self {
            Surface::Lattice { points, .. } => match points.get_mut(index) {
                Some(p) => {
                    *p = to;
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// The radius, chord and depth of an arc of `size`, or `None` for any other
    /// surface — and for an arc so shallow it is a flat panel written the long
    /// way round, whose radius is an infinity nobody wants to read.
    pub fn arc_metrics(&self, size: Vec2) -> Option<ArcMetrics> {
        let Surface::Arc { sweep_deg } = self else {
            return None;
        };
        let theta = sweep_deg.to_radians();
        if theta.abs() < 1e-4 {
            return None;
        }
        let radius = (size.x / theta).abs();
        Some(ArcMetrics {
            radius,
            chord: 2.0 * radius * (theta / 2.0).sin().abs(),
            depth: radius * (1.0 - (theta / 2.0).cos()),
        })
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
            Surface::Lattice { columns, rows, .. } => (
                columns.saturating_sub(1).max(1),
                rows.saturating_sub(1).max(1),
            ),
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
        self.stage_of(self.surface.local_point(u, v, self.placement.size))
    }

    /// The stage-space point that a panel-local point sits at.
    pub fn stage_of(&self, local: Vec3) -> Vec3 {
        self.placement.translation + self.placement.rotation * local
    }

    /// How many cells across and down this panel's surface needs.
    pub fn subdivisions(&self) -> (u32, u32) {
        self.surface.subdivisions()
    }

    /// The panel-local point that a stage-space point sits at.
    ///
    /// The inverse of the pose half of [`Panel::surface_point`], and the step
    /// that turns a dragged handle back into something the surface can store: the
    /// pointer moves in the stage, but a lattice is measured in the panel's own
    /// frame, so a panel that is yawed later carries its shape round with it.
    pub fn local_of(&self, stage: Vec3) -> Vec3 {
        self.placement.rotation.inverse() * (stage - self.placement.translation)
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

    /// Where `point` lands in the frame, as a fraction of it from the **top
    /// left**, or `None` if it is behind the camera.
    ///
    /// Y is flipped on the way out, because clip space has Y up and every screen
    /// this ends up on has Y down. Getting that wrong does not look like a bug —
    /// it looks like a rig that is upside down only while you drag it.
    pub fn project(&self, point: Vec3, aspect: f32) -> Option<Vec2> {
        let clip = self.view_projection(aspect) * point.extend(1.0);
        // Behind the eye, w goes to zero and then negative; dividing through it
        // puts the point back on screen, mirrored, which is worse than losing it.
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new((ndc.x + 1.0) / 2.0, (1.0 - ndc.y) / 2.0))
    }

    /// The ray through the frame at `uv`, a fraction of it from the top left.
    ///
    /// The exact inverse of [`Camera::project`], so a handle picked at a pixel
    /// and dragged from it does not jump on the first frame of the drag.
    pub fn ray(&self, uv: Vec2, aspect: f32) -> Ray {
        let inverse = self.view_projection(aspect).inverse();
        let ndc = Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
        // Depth 0 is the near plane and 1 the far one: this is wgpu's clip space,
        // which is what `perspective_rh` builds.
        let unproject = |depth: f32| {
            let p = inverse * glam::Vec4::new(ndc.x, ndc.y, depth, 1.0);
            p.truncate() / p.w
        };
        let near = unproject(0.0);
        Ray::new(near, unproject(1.0) - near)
    }

    /// The direction the camera looks, which is also the normal of the plane a
    /// handle drags in — the one plane through a handle that never foreshortens
    /// under the pointer.
    pub fn forward(&self) -> Vec3 {
        (self.target - self.position).normalize_or_zero()
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
        let panel = Panel::from_layout(
            "p",
            "P",
            Size::new(400, 200),
            Rect::new(0.0, 0.0, 400.0, 200.0),
            2.6,
        );
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
        let mut panel = Panel::from_layout(
            "p",
            "P",
            Size::new(1000, 200),
            Rect::new(0.0, 0.0, 1000.0, 200.0),
            2.6,
        );
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
        assert!(
            Surface::Arc { sweep_deg: -90.0 }
                .local_point(0.0, 0.5, panel.placement.size)
                .z
                > 0.01
        );

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
        let Surface::Lattice {
            columns,
            rows,
            mut points,
        } = flat
        else {
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
        let broken = Surface::Lattice {
            columns: 4,
            rows: 4,
            points: vec![Vec3::ZERO; 3],
        };
        let size = Vec2::new(4.0, 2.0);
        for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
            assert_eq!(
                broken.local_point(u, v, size),
                Surface::Flat.local_point(u, v, size)
            );
        }
    }

    #[test]
    fn baking_a_lattice_at_its_own_size_changes_nothing() {
        // The inspector re-bakes on every spinner change. If this drifted, a
        // measured surface would erode one nudge at a time.
        let size = Vec2::new(4.0, 2.0);
        let mut points = Surface::flat_lattice(size, 4, 3).unwrap().points().to_vec();
        points[5] += Vec3::new(0.1, -0.2, 0.7);
        let measured = Surface::lattice(4, 3, points).unwrap();

        let again = measured.bake_lattice(size, 4, 3).unwrap();
        assert_eq!(again.lattice_dims(), Some((4, 3)));
        for (a, b) in again.points().iter().zip(measured.points()) {
            assert!((*a - *b).length() < 1e-5, "{a:?} drifted from {b:?}");
        }
    }

    #[test]
    fn baking_an_arc_keeps_the_shape_the_renderer_was_drawing() {
        // Converting to a lattice must not move the picture: this is the moment
        // an operator switches from a parameter to handles, mid-edit.
        let size = Vec2::new(6.0, 2.5);
        let arc = Surface::Arc { sweep_deg: 60.0 };
        let baked = arc.bake_lattice(size, 17, 3).expect("a lattice");

        for i in 0..=16 {
            let u = i as f32 / 16.0;
            let a = arc.local_point(u, 0.5, size);
            let b = baked.local_point(u, 0.5, size);
            // Exact on the control columns; between them the lattice chords the
            // arc, and 60 degrees over 16 cells is well under a millimetre.
            assert!((a - b).length() < 1e-3, "u={u}: arc {a:?} vs baked {b:?}");
        }
        assert!(baked.points().iter().all(|p| p.is_finite()));
    }

    #[test]
    fn baking_refuses_a_degenerate_grid_rather_than_making_an_unusable_surface() {
        let size = Vec2::new(4.0, 2.0);
        assert!(Surface::Flat.bake_lattice(size, 1, 4).is_none());
        assert!(Surface::Flat.bake_lattice(size, 4, 0).is_none());
    }

    #[test]
    fn moving_a_control_point_only_answers_for_a_lattice() {
        let size = Vec2::new(4.0, 2.0);
        let mut lattice = Surface::flat_lattice(size, 3, 3).unwrap();
        assert!(lattice.set_point(4, Vec3::new(0.0, 0.0, 1.0)));
        assert!((lattice.local_point(0.5, 0.5, size).z - 1.0).abs() < 1e-5);

        // A stale index, and a surface with no points at all: both refused, and
        // neither panics — the selection outlives the surface it was made against.
        assert!(!lattice.set_point(99, Vec3::ZERO));
        assert!(!Surface::Arc { sweep_deg: 30.0 }.set_point(0, Vec3::ZERO));
        assert!(Surface::Flat.points().is_empty());
        assert_eq!(Surface::Flat.lattice_dims(), None);
    }

    #[test]
    fn arc_metrics_match_the_geometry_and_a_flat_arc_has_none() {
        let size = Vec2::new(6.0, 2.0);
        let arc = Surface::Arc { sweep_deg: 180.0 };
        let m = arc.arc_metrics(size).expect("a real arc");

        // A half circle of arc length 6: radius 6/pi, chord = the diameter, and
        // the depth is that same radius.
        assert!(
            (m.radius - 6.0 / std::f32::consts::PI).abs() < 1e-4,
            "{m:?}"
        );
        assert!((m.chord - 2.0 * m.radius).abs() < 1e-4, "{m:?}");
        assert!((m.depth - m.radius).abs() < 1e-4, "{m:?}");

        // The chord agrees with where the ends actually are.
        let ends = (arc.local_point(1.0, 0.5, size) - arc.local_point(0.0, 0.5, size)).length();
        assert!(
            (m.chord - ends).abs() < 1e-3,
            "chord {} vs ends {ends}",
            m.chord
        );

        // Sign does not change any of it: a wall bulging forwards is the same
        // circle seen from the other side.
        let back = Surface::Arc { sweep_deg: -180.0 }
            .arc_metrics(size)
            .unwrap();
        assert!((back.radius - m.radius).abs() < 1e-4);
        assert!((back.chord - m.chord).abs() < 1e-4);

        assert_eq!(Surface::Arc { sweep_deg: 0.0 }.arc_metrics(size), None);
        assert_eq!(Surface::Flat.arc_metrics(size), None);
    }

    #[test]
    fn local_of_is_the_inverse_of_the_panels_pose() {
        let mut panel = Panel::from_layout(
            "p",
            "P",
            Size::new(400, 200),
            Rect::new(0.0, 0.0, 400.0, 200.0),
            2.6,
        );
        panel.placement.translation = Vec3::new(-3.0, 4.5, 1.25);
        panel.placement.rotation = Quat::from_rotation_y(0.7);
        panel.surface = Surface::Arc { sweep_deg: 40.0 };

        for (u, v) in [(0.0, 0.0), (0.5, 0.5), (1.0, 0.25), (0.75, 1.0)] {
            let stage = panel.surface_point(u, v);
            let local = panel.local_of(stage);
            let expected = panel.surface.local_point(u, v, panel.placement.size);
            assert!(
                (local - expected).length() < 1e-4,
                "({u},{v}): {local:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn projecting_and_unprojecting_are_the_same_map_read_both_ways() {
        // What makes a dragged handle stay under the pointer: pick a point, and
        // the ray through where it lands must go straight back through it.
        let cam = Camera::default();
        let aspect = 16.0 / 9.0;
        for point in [
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(-2.5, 1.0, -3.0),
            Vec3::new(3.0, 6.0, 2.0),
        ] {
            let uv = cam.project(point, aspect).expect("in front of the camera");
            let ray = cam.ray(uv, aspect);
            let hit = ray
                .intersect_plane(point, cam.forward())
                .expect("the plane through the point faces the camera");
            assert!(
                (hit - point).length() < 1e-3,
                "{point:?} came back as {hit:?}"
            );
        }
    }

    #[test]
    fn the_centre_of_the_frame_is_the_camera_target_and_up_is_up() {
        let cam = Camera::default();
        let centre = cam.project(cam.target, 16.0 / 9.0).unwrap();
        assert!((centre - Vec2::new(0.5, 0.5)).length() < 1e-4, "{centre:?}");

        // Higher in the stage must mean *smaller* v: the flip from clip space to
        // a screen, which is invisible until you drag something.
        let above = cam.project(cam.target + Vec3::Y, 16.0 / 9.0).unwrap();
        assert!(above.y < centre.y, "{above:?} should sit above {centre:?}");
    }

    #[test]
    fn a_point_behind_the_camera_has_no_place_on_screen() {
        let cam = Camera::default();
        // The camera sits at z = 12 looking back at the origin, so this is behind it.
        assert_eq!(cam.project(Vec3::new(0.0, 1.7, 20.0), 16.0 / 9.0), None);
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
