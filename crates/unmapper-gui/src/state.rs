//! Everything the UI edits, and the live NDI plumbing behind it.
//!
//! Kept apart from the widgets so the interesting logic — which sources are
//! connected, what a drag does to a panel — is testable without a window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use unmapper_core::{Camera, Panel, Rect, Show, SourceKind, Surface, Vec2, Vec3};

use crate::outputs::MonitorInfo;
use unmapper_ndi::{Ndi, ReceiverHandle, SourceName};

/// Which view the central viewport is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Canvas,
    Previz,
}

/// A transient message shown in the status bar.
pub struct Toast {
    pub text: String,
    pub is_error: bool,
    pub at: Instant,
}

impl Toast {
    /// How long a message stays up. Long enough to read a warning, short enough
    /// not to hide the next one.
    const LIFETIME: Duration = Duration::from_secs(6);

    pub fn expired(&self) -> bool {
        self.at.elapsed() > Self::LIFETIME
    }
}

/// What is being dragged in the canvas view.
pub enum Drag {
    /// Moving a panel. Holds the grab offset within the panel, in canvas pixels,
    /// so the panel does not jump to centre itself under the cursor.
    Panel { id: String, grab: Vec2 },
    /// Panning the view.
    Pan,
    /// Pulling one of a surface's control points about in the previz view.
    ///
    /// `grab` is the offset from the pointer's position on the drag plane to the
    /// handle, in metres, so a handle picked slightly off-centre does not snap
    /// itself under the cursor the moment the drag starts.
    SurfacePoint {
        panel: String,
        index: usize,
        grab: Vec3,
    },
}

/// Which shape a panel's surface is, without the shape itself.
///
/// The inspector picks a kind and the surface is converted to it. Keeping the
/// choice apart from the data is what lets a conversion *sample* the old shape
/// instead of discarding it — switching an arc to a lattice mid-edit must not
/// move the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Flat,
    Arc,
    Lattice,
}

impl SurfaceKind {
    pub fn of(surface: &Surface) -> Self {
        match surface {
            Surface::Flat => SurfaceKind::Flat,
            Surface::Arc { .. } => SurfaceKind::Arc,
            Surface::Lattice { .. } => SurfaceKind::Lattice,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SurfaceKind::Flat => "Flat",
            SurfaceKind::Arc => "Arc",
            SurfaceKind::Lattice => "Lattice",
        }
    }
}

/// The sweep a panel gets the first time it becomes an arc.
///
/// Deliberately not zero: a zero-sweep arc *is* a flat panel, and an operator
/// who picks "Arc", sees nothing happen and concludes the control is broken is
/// not wrong to.
const NEW_ARC_SWEEP_DEG: f32 = 15.0;

/// The lattice a panel gets the first time it becomes one — wide enough to shape
/// a wall, few enough handles to see which one you are dragging.
const NEW_LATTICE: (u32, u32) = (5, 3);

/// The most control points a lattice may be given from the UI.
///
/// 33 x 33 is over a thousand handles, which is already past the point where
/// dragging one is a sensible way to describe a shape; beyond it the overlay
/// costs more than the render underneath it.
pub const MAX_LATTICE: u32 = 33;

/// Where each of `panel`'s control points lands in the frame, as a fraction of
/// it from the top left, paired with the index it came from.
///
/// Points behind the camera are dropped rather than clamped: a handle that is
/// not on screen must not be pickable, and a clamped one sits on the edge of the
/// viewport looking exactly like one that is.
pub fn handle_positions(panel: &Panel, camera: &Camera, aspect: f32) -> Vec<(usize, Vec2)> {
    panel
        .surface
        .points()
        .iter()
        .enumerate()
        .filter_map(|(i, local)| Some((i, camera.project(panel.stage_of(*local), aspect)?)))
        .collect()
}

/// The handle nearest `at` within `radius`, all three in the same units.
///
/// Nearest rather than first: handles overlap in a 3D view, and the one whose
/// centre is closest to the pointer is the one being aimed at.
pub fn nearest_handle(handles: &[(usize, Vec2)], at: Vec2, radius: f32) -> Option<usize> {
    handles
        .iter()
        .map(|(i, p)| (*i, (*p - at).length()))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

pub struct App {
    pub show: Show,
    pub path: Option<PathBuf>,
    /// Set on every edit, cleared on save — drives the title bar's dirty marker
    /// and the close confirmation.
    pub dirty: bool,

    pub mode: ViewMode,
    pub selected: Option<String>,
    /// Which control point of the selected panel's surface is being edited.
    /// Always an index into *that* panel's surface, and cleared whenever the
    /// surface it indexes into could have changed shape.
    pub selected_point: Option<usize>,
    pub drag: Option<Drag>,

    /// The About window, from the Help menu. See about_window.rs, vendored from
    /// stoatworks-backend/about.
    pub show_about: bool,

    /// Canvas view: pixels of canvas per screen point, and the canvas-space point
    /// at the top-left of the viewport.
    pub zoom: f32,
    pub pan: Vec2,
    /// The viewport's size in physical pixels, published by the viewport widget
    /// each frame. Framing needs it and cannot know it any earlier.
    pub viewport_px: Vec2,
    /// Set when a show is loaded, consumed once the viewport size is known.
    /// Framing at load time would use a stale or zero viewport.
    pub needs_frame: bool,

    /// Previz view: orbit around the rig.
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub orbit_distance: f32,

    /// Connected displays, refreshed on startup and on demand. The UI needs
    /// these to offer a choice; the event loop is the only thing that can read
    /// them, so they are snapshotted here rather than queried from a widget.
    pub monitors: Vec<MonitorInfo>,

    pub ndi: Option<Ndi>,
    pub ndi_error: Option<String>,
    pub discovered: Vec<SourceName>,
    pub discovering: bool,
    receivers: HashMap<String, ReceiverHandle>,

    pub toasts: Vec<Toast>,
}

impl Default for App {
    fn default() -> Self {
        // Loading the NDI runtime is the one thing that can fail at startup and
        // still leave a usable app — a machine with no runtime can lay a rig out
        // perfectly well, it just cannot receive. So the error is kept and shown
        // rather than being fatal.
        let (ndi, ndi_error) = match Ndi::load() {
            Ok(n) => (Some(n), None),
            Err(e) => (None, Some(e.to_string())),
        };

        Self {
            show: Show::default(),
            path: None,
            dirty: false,
            mode: ViewMode::Canvas,
            selected: None,
            selected_point: None,
            drag: None,
            show_about: false,
            zoom: 0.25,
            pan: Vec2::ZERO,
            viewport_px: Vec2::new(1280.0, 720.0),
            needs_frame: false,
            orbit_yaw: 0.0,
            orbit_pitch: 0.15,
            orbit_distance: 12.0,
            monitors: Vec::new(),
            ndi,
            ndi_error,
            discovered: Vec::new(),
            discovering: false,
            receivers: HashMap::new(),
            toasts: Vec::new(),
        }
    }
}

impl App {
    /// An app with no NDI runtime, for tests — the same state a machine with no
    /// runtime installed lands in, which is deliberately a usable one.
    #[cfg(test)]
    pub fn headless() -> Self {
        Self {
            ndi: None,
            ndi_error: None,
            ..Default::default()
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            is_error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.toasts.push(Toast {
            text,
            is_error: true,
            at: Instant::now(),
        });
    }

    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.show.name.clone());
        format!("{}{} — UnMapper", if self.dirty { "• " } else { "" }, name)
    }

    /// Replace the whole show, dropping any receivers the old one owned.
    pub fn replace_show(&mut self, show: Show, path: Option<PathBuf>) {
        self.receivers.clear();
        self.selected = None;
        self.selected_point = None;
        self.drag = None;
        self.show = show;
        self.path = path;
        self.dirty = false;
        self.needs_frame = true;
    }

    /// Zoom and pan so the whole rig fits the viewport, centred.
    ///
    /// Called on load, because landing on a 3840-wide canvas at 1:1 shows one
    /// corner and looks broken. Frames the *panels* rather than the canvas
    /// rectangle, so a rig occupying a corner of a large canvas still fills the
    /// view.
    pub fn frame_canvas(&mut self) {
        let content = self.show.panel_extent().unwrap_or(Rect::new(
            0.0,
            0.0,
            self.show.virtual_raster.width as f32,
            self.show.virtual_raster.height as f32,
        ));
        if content.width <= 0.0 || content.height <= 0.0 {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            return;
        }

        // 0.92 leaves a margin, so panels at the very edge are not flush against
        // the viewport border.
        let fit = (self.viewport_px.x / content.width).min(self.viewport_px.y / content.height);
        self.zoom = (fit * 0.92).clamp(0.02, 8.0);

        let visible = self.viewport_px / self.zoom;
        let centre = Vec2::new(
            content.x + content.width / 2.0,
            content.y + content.height / 2.0,
        );
        self.pan = centre - visible / 2.0;
    }

    /// Pull the orbit camera back far enough to see the whole rig.
    pub fn frame_previz(&mut self) {
        let extent = self
            .show
            .panels
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| p.placement.corners())
            .fold(None::<(glam::Vec3, glam::Vec3)>, |acc, c| {
                Some(match acc {
                    None => (c, c),
                    Some((lo, hi)) => (lo.min(c), hi.max(c)),
                })
            });
        self.orbit_distance = match extent {
            Some((lo, hi)) => ((hi - lo).max_element() * 1.6).max(2.0),
            None => 12.0,
        };
        self.orbit_yaw = 0.0;
        self.orbit_pitch = 0.15;
    }

    /// The point the previz camera orbits — the centre of the rig.
    pub fn orbit_centre(&self) -> glam::Vec3 {
        let enabled: Vec<_> = self.show.panels.iter().filter(|p| p.enabled).collect();
        if enabled.is_empty() {
            return glam::Vec3::new(0.0, 2.0, 0.0);
        }
        let (lo, hi) = enabled.iter().flat_map(|p| p.placement.corners()).fold(
            (glam::Vec3::INFINITY, glam::Vec3::NEG_INFINITY),
            |(lo, hi), c| (lo.min(c), hi.max(c)),
        );
        (lo + hi) / 2.0
    }

    pub fn previz_camera(&self) -> unmapper_core::Camera {
        let centre = self.orbit_centre();
        let (sy, cy) = self.orbit_yaw.sin_cos();
        let (sp, cp) = self.orbit_pitch.sin_cos();
        let offset = glam::Vec3::new(sy * cp, sp, cy * cp) * self.orbit_distance;
        unmapper_core::Camera {
            position: centre + offset,
            target: centre,
            ..Default::default()
        }
    }

    /// The topmost enabled panel containing `point` (canvas space).
    ///
    /// Later panels win, matching the draw order — the one visually on top is the
    /// one you grab.
    pub fn panel_at(&self, point: Vec2) -> Option<String> {
        self.show
            .panels
            .iter()
            .rfind(|p| p.enabled && p.layout.contains(point))
            .map(|p| p.id.clone())
    }

    /// Move a panel so its top-left sits at `top_left`, snapping to whole pixels.
    ///
    /// Snapping matters: a panel at x=417.6 samples half a pixel off across the
    /// whole wall, and there is no reason to allow it.
    pub fn move_panel(&mut self, id: &str, top_left: Vec2) {
        if let Some(p) = self.show.panels.iter_mut().find(|p| p.id == id) {
            p.layout = Rect::new(
                top_left.x.round(),
                top_left.y.round(),
                p.layout.width,
                p.layout.height,
            );
            self.dirty = true;
        }
    }

    /// The panel the inspector is editing.
    pub fn selected_panel(&self) -> Option<&Panel> {
        self.selected.as_ref().and_then(|id| self.show.panel(id))
    }

    pub fn panel_index(&self, id: &str) -> Option<usize> {
        self.show.panels.iter().position(|p| p.id == id)
    }

    /// Select a panel, dropping any control point selected on the last one.
    ///
    /// A point index only means anything against the surface it was picked on,
    /// and index 7 of the panel you just left is index 7 of a different shape.
    pub fn select_panel(&mut self, id: Option<String>) {
        if id != self.selected {
            self.selected_point = None;
        }
        self.selected = id;
    }

    /// Convert a panel's surface to `kind`, keeping the shape where the new kind
    /// can hold it. `false` if there is no such panel, or nothing to do.
    pub fn set_surface_kind(&mut self, panel_id: &str, kind: SurfaceKind) -> bool {
        let Some(index) = self.panel_index(panel_id) else {
            return false;
        };
        let panel = &mut self.show.panels[index];
        if SurfaceKind::of(&panel.surface) == kind {
            return false;
        }
        let size = panel.placement.size;
        let surface = match kind {
            // Flattening throws the shape away — but so does every other reading
            // of "make this panel flat", and the operator asked for it.
            SurfaceKind::Flat => Surface::Flat,
            SurfaceKind::Arc => Surface::Arc {
                sweep_deg: NEW_ARC_SWEEP_DEG,
            },
            SurfaceKind::Lattice => {
                let (columns, rows) = panel.surface.lattice_dims().unwrap_or(NEW_LATTICE);
                match panel.surface.bake_lattice(size, columns, rows) {
                    Some(s) => s,
                    // A panel too small to bake against is not a reason to leave
                    // the operator on a kind they did not choose.
                    None => Surface::flat_lattice(size, NEW_LATTICE.0, NEW_LATTICE.1)
                        .unwrap_or(Surface::Flat),
                }
            }
        };
        panel.surface = surface;
        self.selected_point = None;
        self.dirty = true;
        true
    }

    /// Resample the panel's lattice to `columns` x `rows`, keeping its shape.
    pub fn resize_lattice(&mut self, panel_id: &str, columns: u32, rows: u32) -> bool {
        let Some(index) = self.panel_index(panel_id) else {
            return false;
        };
        let panel = &mut self.show.panels[index];
        let columns = columns.clamp(2, MAX_LATTICE);
        let rows = rows.clamp(2, MAX_LATTICE);
        if panel.surface.lattice_dims() == Some((columns, rows)) {
            return false;
        }
        let Some(resampled) = panel
            .surface
            .bake_lattice(panel.placement.size, columns, rows)
        else {
            return false;
        };
        panel.surface = resampled;
        // The grid the index counted along is gone.
        self.selected_point = None;
        self.dirty = true;
        true
    }

    /// Move one control point to a point in **stage** space — where the pointer
    /// is — storing it in the panel's own frame, where the surface lives.
    pub fn set_surface_point(&mut self, panel_id: &str, index: usize, stage: Vec3) -> bool {
        if !stage.is_finite() {
            return false;
        }
        let Some(i) = self.panel_index(panel_id) else {
            return false;
        };
        let panel = &mut self.show.panels[i];
        let local = panel.local_of(stage);
        if !panel.surface.set_point(index, local) {
            return false;
        }
        self.dirty = true;
        true
    }

    /// The stage-space position of one of a panel's control points.
    pub fn surface_handle(&self, panel_id: &str, index: usize) -> Option<Vec3> {
        let panel = self.show.panel(panel_id)?;
        panel
            .surface
            .points()
            .get(index)
            .map(|p| panel.stage_of(*p))
    }

    /// Connect, disconnect and reconnect receivers so they match the show.
    ///
    /// Called every frame. Cheap when nothing changed, which is the common case —
    /// it only touches the map when a binding actually differs.
    pub fn sync_receivers(&mut self) {
        let Some(ndi) = &self.ndi else { return };

        let wanted: HashMap<String, String> = self
            .show
            .sources
            .iter()
            .filter(|s| s.enabled)
            .filter_map(|s| match &s.kind {
                SourceKind::Ndi { name } if !name.is_empty() => Some((s.id.clone(), name.clone())),
                _ => None,
            })
            .collect();

        // Drop receivers whose source went away or was repointed. Dropping the
        // handle stops its thread.
        self.receivers
            .retain(|id, r| wanted.get(id).is_some_and(|n| *n == r.source));

        for (id, name) in wanted {
            if !self.receivers.contains_key(&id) {
                tracing::info!(source = %id, ndi = %name, "connecting");
                self.receivers
                    .insert(id.clone(), ndi.receive(&name, "UnMapper"));
            }
        }
    }

    pub fn receiver(&self, source_id: &str) -> Option<&ReceiverHandle> {
        self.receivers.get(source_id)
    }

    pub fn receivers(&self) -> impl Iterator<Item = (&String, &ReceiverHandle)> {
        self.receivers.iter()
    }

    pub fn prune_toasts(&mut self) {
        self.toasts.retain(|t| !t.expired());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unmapper_core::{Panel, Size};

    fn app_with_panels() -> App {
        let mut app = App {
            ndi: None,
            ndi_error: None,
            ..Default::default()
        };
        app.show.panels.push(Panel::from_layout(
            "a",
            "A",
            Size::new(100, 100),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            2.6,
        ));
        app.show.panels.push(Panel::from_layout(
            "b",
            "B",
            Size::new(100, 100),
            Rect::new(50.0, 50.0, 100.0, 100.0),
            2.6,
        ));
        app
    }

    #[test]
    fn hit_testing_prefers_the_panel_drawn_on_top() {
        let app = app_with_panels();
        // In the overlap, the later panel wins because it is the one you can see.
        assert_eq!(app.panel_at(Vec2::new(75.0, 75.0)).as_deref(), Some("b"));
        assert_eq!(app.panel_at(Vec2::new(10.0, 10.0)).as_deref(), Some("a"));
        assert_eq!(app.panel_at(Vec2::new(500.0, 500.0)), None);
    }

    #[test]
    fn a_disabled_panel_cannot_be_grabbed() {
        let mut app = app_with_panels();
        app.show.panels[1].enabled = false;
        assert_eq!(app.panel_at(Vec2::new(75.0, 75.0)).as_deref(), Some("a"));
    }

    #[test]
    fn moving_a_panel_snaps_to_whole_pixels_and_keeps_its_size() {
        let mut app = app_with_panels();
        app.move_panel("a", Vec2::new(417.6, -3.2));
        let p = app.show.panel("a").unwrap();
        assert_eq!(p.layout, Rect::new(418.0, -3.0, 100.0, 100.0));
        assert!(app.dirty);
    }

    #[test]
    fn framing_an_empty_show_does_not_produce_a_degenerate_camera() {
        let mut app = App {
            ndi: None,
            ndi_error: None,
            ..Default::default()
        };
        app.frame_previz();
        assert!(app.orbit_distance >= 2.0);
        let cam = app.previz_camera();
        assert!(cam.position.is_finite());
        assert!((cam.position - cam.target).length() > 0.1);
    }

    #[test]
    fn the_orbit_camera_stays_the_right_distance_from_the_rig() {
        let mut app = app_with_panels();
        app.frame_previz();
        for yaw in [0.0, 1.0, 3.0, -2.0] {
            app.orbit_yaw = yaw;
            let cam = app.previz_camera();
            let d = (cam.position - app.orbit_centre()).length();
            assert!(
                (d - app.orbit_distance).abs() < 1e-3,
                "distance drifted to {d} at yaw {yaw}"
            );
        }
    }

    #[test]
    fn framing_fits_the_rig_in_the_viewport_and_centres_it() {
        let mut app = app_with_panels();
        app.viewport_px = Vec2::new(800.0, 600.0);
        app.frame_canvas();

        // Panels span (0,0)..(150,150). At the chosen zoom the whole extent must
        // fit, with the margin.
        let visible = app.viewport_px / app.zoom;
        assert!(visible.x >= 150.0 && visible.y >= 150.0, "rig does not fit");

        // And its centre must land in the middle of the viewport.
        let view_centre = app.pan + visible / 2.0;
        assert!(
            (view_centre - Vec2::new(75.0, 75.0)).length() < 0.5,
            "not centred: {view_centre:?}"
        );
    }

    #[test]
    fn framing_a_rig_with_no_panels_does_not_divide_by_zero() {
        let mut app = App {
            ndi: None,
            ndi_error: None,
            ..Default::default()
        };
        app.viewport_px = Vec2::new(800.0, 600.0);
        app.frame_canvas();
        assert!(app.zoom.is_finite() && app.zoom > 0.0);
        assert!(app.pan.is_finite());
    }

    #[test]
    fn loading_defers_framing_until_the_viewport_size_is_known() {
        // Framing at load would use a stale viewport, so it is requested and
        // performed by the viewport widget instead.
        let mut app = app_with_panels();
        app.replace_show(Show::default(), None);
        assert!(app.needs_frame);
    }

    #[test]
    fn becoming_an_arc_actually_bends_the_panel() {
        let mut app = app_with_panels();
        assert!(app.set_surface_kind("a", SurfaceKind::Arc));
        let panel = app.show.panel("a").unwrap();
        assert_eq!(SurfaceKind::of(&panel.surface), SurfaceKind::Arc);
        // A zero-sweep arc would look exactly like the flat panel it replaced.
        let ends_z = panel.surface_point(0.0, 0.5).z;
        let middle_z = panel.surface_point(0.5, 0.5).z;
        assert!(
            ends_z < middle_z - 1e-3,
            "the arc is flat: {ends_z} vs {middle_z}"
        );
        assert!(app.dirty);

        // Asking for the kind it already is changes nothing.
        assert!(!app.set_surface_kind("a", SurfaceKind::Arc));
        assert!(!app.set_surface_kind("nonexistent", SurfaceKind::Flat));
    }

    #[test]
    fn becoming_a_lattice_keeps_the_arc_it_came_from() {
        // The conversion happens mid-edit, in front of the operator: the picture
        // must not move on the frame they pick "Lattice".
        let mut app = app_with_panels();
        app.set_surface_kind("a", SurfaceKind::Arc);
        let before: Vec<_> = (0..=8)
            .map(|i| {
                app.show
                    .panel("a")
                    .unwrap()
                    .surface_point(i as f32 / 8.0, 0.5)
            })
            .collect();

        assert!(app.set_surface_kind("a", SurfaceKind::Lattice));
        let panel = app.show.panel("a").unwrap();
        assert_eq!(panel.surface.lattice_dims(), Some(NEW_LATTICE));
        for (i, was) in before.iter().enumerate() {
            let now = panel.surface_point(i as f32 / 8.0, 0.5);
            assert!((now - *was).length() < 5e-3, "u={i}/8: {now:?} vs {was:?}");
        }
    }

    #[test]
    fn resizing_a_lattice_keeps_its_shape_and_clears_the_stale_selection() {
        let mut app = app_with_panels();
        app.set_surface_kind("a", SurfaceKind::Lattice);
        let handle = app.surface_handle("a", 7).unwrap();
        app.set_surface_point("a", 7, handle + Vec3::new(0.0, 0.0, 0.4));
        app.selected_point = Some(7);
        let pulled = app.show.panel("a").unwrap().surface_point(0.5, 0.5);

        assert!(app.resize_lattice("a", 9, 5));
        let panel = app.show.panel("a").unwrap();
        assert_eq!(panel.surface.lattice_dims(), Some((9, 5)));
        assert!(
            (panel.surface_point(0.5, 0.5) - pulled).length() < 1e-3,
            "the pulled centre moved"
        );
        // Index 7 counted along the old grid and means something else on the new one.
        assert_eq!(app.selected_point, None);

        // Out-of-range sizes are clamped, not refused into a broken lattice.
        app.resize_lattice("a", 0, 9999);
        let dims = app.show.panel("a").unwrap().surface.lattice_dims().unwrap();
        assert_eq!(dims, (2, MAX_LATTICE));
    }

    #[test]
    fn a_dragged_handle_is_stored_in_the_panels_own_frame() {
        // The pointer moves in the stage; the surface is measured in the panel.
        // A panel yawed 90 degrees is where that difference stops being invisible.
        let mut app = app_with_panels();
        app.show.panels[0].placement.translation = Vec3::new(2.0, 3.0, -1.0);
        app.show.panels[0].placement.rotation =
            glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        app.set_surface_kind("a", SurfaceKind::Lattice);

        let target = app.surface_handle("a", 4).unwrap() + Vec3::new(0.3, -0.2, 0.5);
        assert!(app.set_surface_point("a", 4, target));
        let back = app.surface_handle("a", 4).unwrap();
        assert!((back - target).length() < 1e-4, "{back:?} vs {target:?}");
        assert!(app.dirty);

        // And the stored point is not simply the stage point written down.
        let local = app.show.panel("a").unwrap().surface.points()[4];
        assert!((local - target).length() > 0.1, "stored in the wrong space");
    }

    #[test]
    fn a_handle_that_cannot_be_placed_is_refused_rather_than_poisoning_the_surface() {
        let mut app = app_with_panels();
        app.set_surface_kind("a", SurfaceKind::Lattice);
        // A ray that missed its plane can hand back a NaN; one of those in the
        // lattice takes the whole panel out of the render, silently.
        assert!(!app.set_surface_point("a", 0, Vec3::new(f32::NAN, 0.0, 0.0)));
        assert!(!app.set_surface_point("a", 999, Vec3::ZERO));
        assert!(
            !app.set_surface_point("b", 0, Vec3::ZERO),
            "b is still flat"
        );
        assert!(app
            .show
            .panel("a")
            .unwrap()
            .surface
            .points()
            .iter()
            .all(|p| p.is_finite()));
    }

    #[test]
    fn handles_project_where_the_camera_sees_them_and_never_behind_it() {
        let mut app = app_with_panels();
        app.show.panels[0].placement.translation = Vec3::new(0.0, 2.0, 0.0);
        app.set_surface_kind("a", SurfaceKind::Lattice);
        let panel = app.show.panel("a").unwrap();

        let camera = unmapper_core::Camera {
            position: Vec3::new(0.0, 2.0, 6.0),
            target: Vec3::new(0.0, 2.0, 0.0),
            ..Default::default()
        };
        let handles = handle_positions(panel, &camera, 16.0 / 9.0);
        assert_eq!(handles.len(), panel.surface.points().len());
        // The centre control point of a centred panel sits in the middle of frame.
        let centre = handles.iter().find(|(i, _)| *i == 7).unwrap().1;
        assert!((centre - Vec2::new(0.5, 0.5)).length() < 1e-3, "{centre:?}");

        // Standing inside the wall, looking away: nothing is pickable.
        let behind = unmapper_core::Camera {
            position: Vec3::new(0.0, 2.0, -1.0),
            target: Vec3::new(0.0, 2.0, -9.0),
            ..Default::default()
        };
        assert!(handle_positions(panel, &behind, 16.0 / 9.0).is_empty());
    }

    #[test]
    fn picking_takes_the_nearest_handle_inside_the_radius_and_none_outside_it() {
        let handles = vec![
            (3, Vec2::new(0.50, 0.50)),
            (4, Vec2::new(0.52, 0.50)),
            (5, Vec2::new(0.90, 0.90)),
        ];
        // Overlapping handles: the one whose centre is closest is the one aimed at.
        assert_eq!(
            nearest_handle(&handles, Vec2::new(0.515, 0.50), 0.05),
            Some(4)
        );
        assert_eq!(
            nearest_handle(&handles, Vec2::new(0.495, 0.50), 0.05),
            Some(3)
        );
        // Empty space starts an orbit instead, which is what None means here.
        assert_eq!(nearest_handle(&handles, Vec2::new(0.10, 0.10), 0.05), None);
        assert_eq!(nearest_handle(&[], Vec2::new(0.5, 0.5), 0.05), None);
    }

    #[test]
    fn changing_panel_drops_the_point_selected_on_the_last_one() {
        let mut app = app_with_panels();
        app.select_panel(Some("a".into()));
        app.selected_point = Some(4);
        // Re-selecting the same panel is not a change and must not fight the drag.
        app.select_panel(Some("a".into()));
        assert_eq!(app.selected_point, Some(4));

        app.select_panel(Some("b".into()));
        assert_eq!(app.selected_point, None);
        assert_eq!(app.selected.as_deref(), Some("b"));
    }

    #[test]
    fn the_title_marks_unsaved_changes() {
        let mut app = app_with_panels();
        app.show.name = "Wembley".into();
        assert!(!app.title().starts_with('•'));
        app.move_panel("a", Vec2::ZERO);
        assert!(app.title().starts_with('•'), "got {}", app.title());
    }

    #[test]
    fn replacing_the_show_clears_selection_and_dirt() {
        let mut app = app_with_panels();
        app.selected = Some("a".into());
        app.dirty = true;
        app.replace_show(Show::default(), Some("/tmp/x.xml".into()));
        assert!(app.selected.is_none());
        assert!(!app.dirty);
        assert_eq!(
            app.path.as_deref(),
            Some(std::path::Path::new("/tmp/x.xml"))
        );
    }
}
