//! The show file: everything UnMapper needs to reproduce a stage.
//!
//! # The one thing to understand
//!
//! A slice's pixels can be found in *two different places* depending on what the
//! NDI sender is sending, and getting this wrong is the difference between a wall
//! that looks right and a wall showing the wrong quarter of the composition:
//!
//! - Resolume sending its **composition** (one NDI feed for everything) — a
//!   slice's pixels are at its `input` quad.
//! - Resolume sending **one output per screen** (the usual show configuration) —
//!   that feed already has the slicing applied, so a slice's pixels are at its
//!   `output` quad within its own screen's feed.
//!
//! [`SourceSpace`] records which, per source, and [`Show::from_slice_map`] assumes
//! the second because it is what a real rig does.

use serde::{Deserialize, Serialize};

use crate::geom::{Quad, Rect, Vec2};
use crate::slicemap::{Size, SliceMap};
use crate::stage::{Camera, Panel, StageGeometry, DEFAULT_PITCH_MM};
use crate::warp::WarpMesh;

/// Bumped when the on-disk shape changes incompatibly.
pub const SHOW_FORMAT: u32 = 1;

/// Gap left between screens when laying them out on a fresh canvas, in pixels.
const SCREEN_GUTTER: f32 = 64.0;

/// What a source's frames contain, and therefore which quad samples them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "kebab-case")]
pub enum SourceSpace {
    /// The whole Resolume composition. Sample with the slice's `input` quad.
    Composition,
    /// One screen's post-slicing output raster. Sample with the `output` quad.
    ScreenRaster { screen_id: String },
}

/// Where frames come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceKind {
    /// An NDI sender, by its full name (`MACHINE (Arena - Screen 1)`).
    Ndi { name: String },
    /// A built-in pattern, so the geometry can be checked with no Resolume running.
    TestPattern,
    /// A still image, for laying a rig out ahead of a show.
    Still { path: std::path::PathBuf },
}

/// One input feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub space: SourceSpace,
    /// The frame size expected, when known from the slice map. Used to turn a
    /// pixel-space quad into texture coordinates before the first frame lands.
    pub expected: Option<Size>,
    pub enabled: bool,
}

/// Ties one panel to the region of one source that feeds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub panel_id: String,
    pub source_id: String,
    /// The region to sample, in the source's own pixel space. Which of the
    /// slice's two quads this came from is decided by the source's [`SourceSpace`].
    pub source_quad: Quad,
    /// The warp lattice deforming that region, in the same space as
    /// [`Binding::source_quad`].
    ///
    /// `None` — the overwhelmingly common case — means the region is just the
    /// quad, and the renderer draws two triangles as it always has. See
    /// [`crate::warp`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_mesh: Option<WarpMesh>,
    /// The slice this came from, kept so a re-import can update it in place.
    pub slice_id: Option<String>,
}

/// Where a rendered view is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "kebab-case")]
pub enum OutputTarget {
    /// A connected monitor, by index into the platform's monitor list.
    Display {
        index: usize,
        fullscreen: bool,
    },
    Ndi {
        name: String,
    },
    /// macOS only.
    Syphon {
        name: String,
    },
    /// Windows only.
    Spout {
        name: String,
    },
}

/// What a given output shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "kebab-case")]
pub enum OutputView {
    /// A pixel-exact crop of the emulation canvas. This is the mode where a
    /// monitor stands in for a piece of the real wall: one canvas pixel is one
    /// LED, and `region` says which piece this monitor is.
    Emulation { region: Rect },
    /// A rendered camera view of the 3D stage.
    Previz { camera: Camera },
}

/// One rendered output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub id: String,
    pub name: String,
    pub target: OutputTarget,
    pub view: OutputView,
    /// Render size. Ignored for `Display` targets, which use the monitor's size.
    pub size: Size,
    pub enabled: bool,
}

/// A whole show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Show {
    pub format: u32,
    pub name: String,
    /// The emulation canvas — the virtual recreation of the whole LED rig.
    pub virtual_raster: Size,
    /// The imported slice map, kept so panels can be re-derived after a re-import.
    pub slice_map: Option<SliceMap>,
    pub sources: Vec<Source>,
    pub panels: Vec<Panel>,
    pub bindings: Vec<Binding>,
    pub geometry: StageGeometry,
    pub outputs: Vec<Output>,
}

impl Default for Show {
    fn default() -> Self {
        Self {
            format: SHOW_FORMAT,
            name: "Untitled".into(),
            virtual_raster: Size::new(1920, 1080),
            slice_map: None,
            sources: Vec::new(),
            panels: Vec::new(),
            bindings: Vec::new(),
            geometry: StageGeometry::default(),
            outputs: Vec::new(),
        }
    }
}

/// Something wrong enough with a show to be worth telling the operator about.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Will not render correctly.
    Error,
    /// Will render, but probably not what was meant.
    Warning,
}

impl Show {
    /// Build a show from a freshly imported slice map.
    ///
    /// One source per screen (matching a rig where Resolume sends each output as
    /// its own NDI feed), one panel per slice, and the screens laid out down the
    /// canvas in import order. Everything here is a starting point the operator
    /// rearranges — the point is that loading a slice map immediately gives
    /// something that renders, rather than an empty canvas and a lot of typing.
    pub fn from_slice_map(map: SliceMap, pitch_mm: f32) -> Self {
        let mut show = Show {
            name: map.project_name.clone(),
            ..Default::default()
        };

        let mut cursor_y = 0.0f32;
        let mut canvas_w = 0.0f32;

        for screen in &map.screens {
            let source_id = format!("src-{}", screen.id);
            show.sources.push(Source {
                id: source_id.clone(),
                name: screen.name.clone(),
                // The screen's own name is what Resolume puts in the NDI sender
                // name, but only the operator can confirm the machine prefix, so
                // this starts unbound rather than guessing a name that will not
                // resolve.
                kind: SourceKind::TestPattern,
                space: SourceSpace::ScreenRaster {
                    screen_id: screen.id.clone(),
                },
                expected: Some(screen.raster),
                enabled: true,
            });

            for slice in &screen.slices {
                let bounds = slice.output.bounds();
                // Panels are laid out at their position within the screen raster,
                // with each screen offset below the last, so a multi-screen rig
                // arrives readable instead of with every screen stacked at 0,0.
                let layout = Rect::new(bounds.x, bounds.y + cursor_y, bounds.width, bounds.height);
                let pixels = Size::new(
                    bounds.width.round().max(1.0) as u32,
                    bounds.height.round().max(1.0) as u32,
                );

                let panel_id = format!("panel-{}-{}", screen.id, slice.id);
                show.panels.push(Panel::from_layout(
                    panel_id.clone(),
                    slice.name.clone(),
                    pixels,
                    layout,
                    pitch_mm,
                ));
                show.bindings.push(Binding {
                    panel_id,
                    source_id: source_id.clone(),
                    source_quad: slice.output,
                    source_mesh: slice.warp.clone(),
                    slice_id: Some(slice.id.clone()),
                });
            }

            cursor_y += screen.raster.height as f32 + SCREEN_GUTTER;
            canvas_w = canvas_w.max(screen.raster.width as f32);
        }

        show.virtual_raster = Size::new(
            canvas_w.max(1.0) as u32,
            (cursor_y - SCREEN_GUTTER).max(1.0) as u32,
        );
        show.arrange_panels_from_layout();
        show.slice_map = Some(map);
        show
    }

    /// Push every panel's 3D pose to match its 2D layout, on a flat plane.
    ///
    /// The canvas has Y down and the stage has Y up, so this is also where that
    /// flip is handled — a panel at the top of the canvas must end up at the top
    /// of the wall, not upside down at the bottom.
    pub fn arrange_panels_from_layout(&mut self) {
        let canvas = self.virtual_raster.as_vec2();
        for panel in &mut self.panels {
            let pitch = panel.pitch_mm().unwrap_or(DEFAULT_PITCH_MM) / 1000.0;
            let centre_px = Vec2::new(
                panel.layout.x + panel.layout.width / 2.0,
                panel.layout.y + panel.layout.height / 2.0,
            );
            panel.placement.size =
                Vec2::new(panel.layout.width * pitch, panel.layout.height * pitch);
            panel.placement.translation = glam::Vec3::new(
                (centre_px.x - canvas.x / 2.0) * pitch,
                (canvas.y - centre_px.y) * pitch,
                0.0,
            );
        }
    }

    pub fn panel(&self, id: &str) -> Option<&Panel> {
        self.panels.iter().find(|p| p.id == id)
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// The smallest canvas that contains every panel.
    pub fn panel_extent(&self) -> Option<Rect> {
        self.panels
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.layout)
            .reduce(|a, b| a.union(&b))
    }

    /// Re-apply a re-imported slice map to the existing panels.
    ///
    /// Bindings are matched by `slice_id`, so an operator who has spent an hour
    /// placing panels and then re-exports the Advanced Output keeps that work.
    /// Slices that are new arrive as new panels; slices that vanished leave their
    /// panels behind, disabled rather than deleted, since deleting someone's
    /// placement because a slice was renamed would be unforgivable.
    pub fn reapply_slice_map(&mut self, map: SliceMap, pitch_mm: f32) -> ReapplyReport {
        let mut report = ReapplyReport::default();

        for (screen, slice) in map.slices() {
            let existing = self
                .bindings
                .iter_mut()
                .find(|b| b.slice_id.as_deref() == Some(slice.id.as_str()));

            match existing {
                Some(binding) => {
                    binding.source_quad = slice.output;
                    // A re-import is the authority on the warp too, including a
                    // warp the operator has just removed in Resolume — so this
                    // assigns rather than merging, and clears back to None.
                    binding.source_mesh = slice.warp.clone();
                    report.updated += 1;
                }
                None => {
                    let bounds = slice.output.bounds();
                    let panel_id = format!("panel-{}-{}", screen.id, slice.id);
                    let source_id = self
                        .sources
                        .iter()
                        .find(|s| {
                            s.space
                                == SourceSpace::ScreenRaster {
                                    screen_id: screen.id.clone(),
                                }
                        })
                        .map(|s| s.id.clone())
                        .unwrap_or_else(|| format!("src-{}", screen.id));

                    self.panels.push(Panel::from_layout(
                        panel_id.clone(),
                        slice.name.clone(),
                        Size::new(
                            bounds.width.round().max(1.0) as u32,
                            bounds.height.round().max(1.0) as u32,
                        ),
                        bounds,
                        pitch_mm,
                    ));
                    self.bindings.push(Binding {
                        panel_id,
                        source_id,
                        source_quad: slice.output,
                        source_mesh: slice.warp.clone(),
                        slice_id: Some(slice.id.clone()),
                    });
                    report.added += 1;
                }
            }
        }

        let live: Vec<String> = map.slices().map(|(_, s)| s.id.clone()).collect();
        for binding in &self.bindings {
            if let Some(sid) = &binding.slice_id {
                if !live.contains(sid) {
                    if let Some(panel) = self.panels.iter_mut().find(|p| p.id == binding.panel_id) {
                        if panel.enabled {
                            panel.enabled = false;
                            report.orphaned += 1;
                        }
                    }
                }
            }
        }

        self.slice_map = Some(map);
        report
    }

    /// Everything wrong with the show that is worth saying out loud.
    pub fn validate(&self) -> Vec<Problem> {
        let mut out = Vec::new();

        for binding in &self.bindings {
            if self.panel(&binding.panel_id).is_none() {
                out.push(Problem {
                    severity: Severity::Error,
                    message: format!("binding refers to missing panel {}", binding.panel_id),
                });
            }
            if self.source(&binding.source_id).is_none() {
                out.push(Problem {
                    severity: Severity::Error,
                    message: format!("binding refers to missing source {}", binding.source_id),
                });
            }
        }

        for panel in self.panels.iter().filter(|p| p.enabled) {
            if !self.bindings.iter().any(|b| b.panel_id == panel.id) {
                out.push(Problem {
                    severity: Severity::Warning,
                    message: format!(
                        "panel \"{}\" has no source bound; it will render black",
                        panel.name
                    ),
                });
            }
        }

        // A panel hanging off the canvas renders fine in previz and is invisible
        // in emulation, which is a confusing way to lose a wall.
        let canvas = Rect::new(
            0.0,
            0.0,
            self.virtual_raster.width as f32,
            self.virtual_raster.height as f32,
        );
        for panel in self.panels.iter().filter(|p| p.enabled) {
            if panel.layout.intersect(&canvas).is_none() {
                out.push(Problem {
                    severity: Severity::Warning,
                    message: format!(
                        "panel \"{}\" lies entirely outside the {}x{} canvas",
                        panel.name, self.virtual_raster.width, self.virtual_raster.height
                    ),
                });
            }
        }

        for output in self.outputs.iter().filter(|o| o.enabled) {
            if let OutputView::Emulation { region } = &output.view {
                if region.is_empty() {
                    out.push(Problem {
                        severity: Severity::Error,
                        message: format!("output \"{}\" has an empty region", output.name),
                    });
                } else if region.intersect(&canvas).is_none() {
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!(
                            "output \"{}\" crops a region entirely outside the canvas; it will \
                             show nothing but the canvas edge",
                            output.name
                        ),
                    });
                } else if region.right() > canvas.right() || region.bottom() > canvas.bottom() {
                    // Sampling past the edge clamps rather than wrapping, so the
                    // output shows a smear of edge pixels — wrong in a way that
                    // is easy to mistake for a mapping problem.
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!(
                            "output \"{}\" crops past the edge of the {}x{} canvas",
                            output.name, self.virtual_raster.width, self.virtual_raster.height
                        ),
                    });
                }
            }
            if cfg!(not(target_os = "macos")) {
                if let OutputTarget::Syphon { .. } = output.target {
                    out.push(Problem {
                        severity: Severity::Error,
                        message: format!(
                            "output \"{}\" uses Syphon, which is macOS only",
                            output.name
                        ),
                    });
                }
            }
            if cfg!(not(target_os = "windows")) {
                if let OutputTarget::Spout { .. } = output.target {
                    out.push(Problem {
                        severity: Severity::Error,
                        message: format!(
                            "output \"{}\" uses Spout, which is Windows only",
                            output.name
                        ),
                    });
                }
            }
        }

        out
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(text: &str) -> Result<Self, ShowError> {
        let show: Show = serde_json::from_str(text)?;
        if show.format > SHOW_FORMAT {
            return Err(ShowError::TooNew {
                found: show.format,
                supported: SHOW_FORMAT,
            });
        }
        Ok(show)
    }
}

/// What a re-import changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReapplyReport {
    pub updated: usize,
    pub added: usize,
    pub orphaned: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ShowError {
    #[error("that show file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "that show was written by a newer UnMapper (format {found}, this build reads {supported})"
    )]
    TooNew { found: u32, supported: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slicemap::{RasterSource, Screen, Slice};

    fn slice(id: &str, r: Rect) -> Slice {
        Slice {
            id: id.into(),
            name: format!("Slice {id}"),
            input: Quad::from_rect(r),
            output: Quad::from_rect(r),
            enabled: true,
            orientation: 0,
            warp: None,
        }
    }

    fn two_screen_map() -> SliceMap {
        SliceMap {
            project_name: "Test Rig".into(),
            composition: Some(Size::new(1920, 1080)),
            screens: vec![
                Screen {
                    id: "A".into(),
                    name: "Upstage".into(),
                    raster: Size::new(1920, 1080),
                    raster_source: RasterSource::Declared,
                    device: None,
                    slices: vec![
                        slice("a1", Rect::new(0.0, 0.0, 960.0, 1080.0)),
                        slice("a2", Rect::new(960.0, 0.0, 960.0, 1080.0)),
                    ],
                    notes: vec![],
                },
                Screen {
                    id: "B".into(),
                    name: "Side".into(),
                    raster: Size::new(1280, 720),
                    raster_source: RasterSource::SliceBounds,
                    device: None,
                    slices: vec![slice("b1", Rect::new(0.0, 0.0, 1280.0, 720.0))],
                    notes: vec![],
                },
            ],
            warnings: vec![],
        }
    }

    #[test]
    fn import_builds_a_panel_per_slice_and_a_source_per_screen() {
        let show = Show::from_slice_map(two_screen_map(), 2.6);
        assert_eq!(show.panels.len(), 3);
        assert_eq!(show.sources.len(), 2);
        assert_eq!(show.bindings.len(), 3);
        // Every binding resolves.
        assert!(show
            .validate()
            .iter()
            .all(|p| p.severity != Severity::Error));
    }

    #[test]
    fn screens_do_not_land_on_top_of_each_other() {
        let show = Show::from_slice_map(two_screen_map(), 2.6);
        let a1 = show.panel("panel-A-a1").unwrap();
        let b1 = show.panel("panel-B-b1").unwrap();
        assert_eq!(a1.layout.y, 0.0);
        assert_eq!(b1.layout.y, 1080.0 + SCREEN_GUTTER);
        assert!(a1.layout.intersect(&b1.layout).is_none());
    }

    #[test]
    fn bindings_sample_the_output_quad_because_resolume_sends_per_screen() {
        let show = Show::from_slice_map(two_screen_map(), 2.6);
        let b = show
            .bindings
            .iter()
            .find(|b| b.slice_id.as_deref() == Some("a2"))
            .unwrap();
        // a2's output quad starts at x=960 within its screen raster.
        assert_eq!(b.source_quad.tl, Vec2::new(960.0, 0.0));
        let src = show.source(&b.source_id).unwrap();
        assert_eq!(
            src.space,
            SourceSpace::ScreenRaster {
                screen_id: "A".into()
            }
        );
    }

    #[test]
    fn canvas_top_maps_to_stage_top_not_upside_down() {
        let mut map = two_screen_map();
        map.screens.truncate(1);
        let show = Show::from_slice_map(map, 2.6);
        // Both panels are full height, so compare a tall panel against the canvas:
        // the panel centre should be above the deck, never below it.
        for p in &show.panels {
            assert!(
                p.placement.translation.y > 0.0,
                "panel {} sank below the deck at y={}",
                p.name,
                p.placement.translation.y
            );
        }
    }

    #[test]
    fn json_round_trip_is_lossless() {
        let show = Show::from_slice_map(two_screen_map(), 2.6);
        let text = show.to_json().unwrap();
        let back = Show::from_json(&text).unwrap();
        assert_eq!(show, back);
    }

    #[test]
    fn a_show_from_the_future_is_refused_rather_than_half_read() {
        let show = Show {
            format: SHOW_FORMAT + 1,
            ..Default::default()
        };
        let text = show.to_json().unwrap();
        assert!(matches!(
            Show::from_json(&text),
            Err(ShowError::TooNew { .. })
        ));
    }

    #[test]
    fn reimport_keeps_placement_work() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        // Operator drags a panel somewhere deliberate.
        let moved = Rect::new(4000.0, 250.0, 960.0, 1080.0);
        show.panels
            .iter_mut()
            .find(|p| p.id == "panel-A-a1")
            .unwrap()
            .layout = moved;

        let report = show.reapply_slice_map(two_screen_map(), 2.6);
        assert_eq!(report.updated, 3);
        assert_eq!(report.added, 0);
        assert_eq!(report.orphaned, 0);
        assert_eq!(show.panel("panel-A-a1").unwrap().layout, moved);
    }

    #[test]
    fn a_slice_that_disappears_disables_its_panel_rather_than_deleting_it() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        let mut shrunk = two_screen_map();
        shrunk.screens[0].slices.retain(|s| s.id != "a2");

        let report = show.reapply_slice_map(shrunk, 2.6);
        assert_eq!(report.orphaned, 1);
        let panel = show.panel("panel-A-a2").expect("panel must still exist");
        assert!(!panel.enabled);
    }

    #[test]
    fn a_new_slice_arrives_as_a_new_panel() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        let mut grown = two_screen_map();
        grown.screens[1]
            .slices
            .push(slice("b2", Rect::new(0.0, 720.0, 1280.0, 360.0)));

        let report = show.reapply_slice_map(grown, 2.6);
        assert_eq!(report.added, 1);
        assert!(show.panel("panel-B-b2").is_some());
        assert!(show
            .validate()
            .iter()
            .all(|p| p.severity != Severity::Error));
    }

    #[test]
    fn validate_flags_an_unbound_panel() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        show.bindings.retain(|b| b.panel_id != "panel-A-a1");
        let problems = show.validate();
        assert!(problems
            .iter()
            .any(|p| p.severity == Severity::Warning && p.message.contains("no source bound")));
    }

    #[test]
    fn validate_flags_an_output_cropping_past_the_canvas_edge() {
        // Sampling past the edge clamps, so the monitor shows a smear of edge
        // pixels rather than failing — easy to misread as a mapping problem.
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        show.outputs.push(Output {
            id: "o".into(),
            name: "Overhanging".into(),
            target: OutputTarget::Display {
                index: 0,
                fullscreen: false,
            },
            view: OutputView::Emulation {
                region: Rect::new(0.0, 0.0, show.virtual_raster.width as f32 + 500.0, 100.0),
            },
            size: Size::new(1920, 1080),
            enabled: true,
        });
        assert!(show
            .validate()
            .iter()
            .any(|p| p.message.contains("crops past the edge")));
    }

    #[test]
    fn validate_flags_an_output_entirely_off_the_canvas() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        show.outputs.push(Output {
            id: "o".into(),
            name: "Lost".into(),
            target: OutputTarget::Display {
                index: 0,
                fullscreen: false,
            },
            view: OutputView::Emulation {
                region: Rect::new(90_000.0, 90_000.0, 100.0, 100.0),
            },
            size: Size::new(1920, 1080),
            enabled: true,
        });
        assert!(show
            .validate()
            .iter()
            .any(|p| p.message.contains("entirely outside the canvas")));
    }

    #[test]
    fn an_output_cropping_exactly_the_canvas_is_not_flagged() {
        // The single-wall case: one output showing the whole rig. Warning on
        // this would cry wolf on the most ordinary configuration there is.
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        show.outputs.push(Output {
            id: "o".into(),
            name: "Whole wall".into(),
            target: OutputTarget::Display {
                index: 0,
                fullscreen: true,
            },
            view: OutputView::Emulation {
                region: Rect::new(
                    0.0,
                    0.0,
                    show.virtual_raster.width as f32,
                    show.virtual_raster.height as f32,
                ),
            },
            size: show.virtual_raster,
            enabled: true,
        });
        assert!(!show.validate().iter().any(|p| p.message.contains("crops")));
    }

    #[test]
    fn validate_flags_a_panel_dragged_off_the_canvas() {
        let mut show = Show::from_slice_map(two_screen_map(), 2.6);
        show.panels
            .iter_mut()
            .find(|p| p.id == "panel-A-a1")
            .unwrap()
            .layout = Rect::new(90_000.0, 90_000.0, 100.0, 100.0);
        assert!(show
            .validate()
            .iter()
            .any(|p| p.message.contains("outside the")));
    }
}
