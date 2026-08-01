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
            enabled: true,
        }
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
}
