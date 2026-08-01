//! Everything the UI edits, and the live NDI plumbing behind it.
//!
//! Kept apart from the widgets so the interesting logic — which sources are
//! connected, what a drag does to a panel — is testable without a window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use unmapper_core::{Rect, Show, SourceKind, Vec2};

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
}

pub struct App {
    pub show: Show,
    pub path: Option<PathBuf>,
    /// Set on every edit, cleared on save — drives the title bar's dirty marker
    /// and the close confirmation.
    pub dirty: bool,

    pub mode: ViewMode,
    pub selected: Option<String>,
    pub drag: Option<Drag>,

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
            drag: None,
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
