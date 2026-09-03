//! The widgets.

use std::path::PathBuf;

use egui::{Color32, RichText};
use unmapper_core::{
    Camera, Output, OutputTarget, OutputView, Panel, Rect, Severity, Show, Size, SourceKind,
    Surface, Vec2, Vec3,
};

use crate::outputs::MonitorInfo;

use crate::state::{
    handle_positions, nearest_handle, App, Drag, SurfaceKind, ViewMode, MAX_LATTICE,
};

/// How close the pointer must come to a control point to grab it, in points.
///
/// Generous next to the 4-point dot it picks: handles are small on purpose, and
/// a wall seen edge-on stacks a whole column of them within a few pixels.
const HANDLE_PICK_RADIUS: f32 = 10.0;

/// The selected panel's surface, drawn over the previz image.
const WIRE: Color32 = Color32::from_rgb(110, 190, 255);
const HANDLE: Color32 = Color32::from_rgb(230, 240, 255);
const HANDLE_SELECTED: Color32 = Color32::from_rgb(255, 170, 60);

/// What the UI is asking the host to do, when it cannot do it itself.
#[derive(Default)]
pub struct Actions {
    pub import_resolume: bool,
    pub open_stage: bool,
    pub save: bool,
    pub save_as: bool,
    pub discover: bool,
    pub rescan_displays: bool,
    pub pick_backdrop: bool,
    pub pick_model: bool,
    pub quit: bool,
}

pub fn menu_bar(ui: &mut egui::Ui, app: &mut App, actions: &mut Actions) {
    egui::containers::Panel::top("menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Import Resolume Advanced Output…").clicked() {
                    actions.import_resolume = true;
                    ui.close();
                }
                if ui.button("Open Stage…").clicked() {
                    actions.open_stage = true;
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(app.path.is_some(), egui::Button::new("Save"))
                    .clicked()
                {
                    actions.save = true;
                    ui.close();
                }
                if ui.button("Save Stage As…").clicked() {
                    actions.save_as = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    actions.quit = true;
                    ui.close();
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("About UnMapper").clicked() {
                    app.show_about = true;
                    ui.close();
                }
            });

            ui.separator();
            ui.selectable_value(&mut app.mode, ViewMode::Canvas, "Emulation");
            ui.selectable_value(&mut app.mode, ViewMode::Previz, "Previz");

            ui.separator();
            if ui
                .button("Frame")
                .on_hover_text("Fit the whole rig in view")
                .clicked()
            {
                match app.mode {
                    ViewMode::Canvas => app.frame_canvas(),
                    ViewMode::Previz => app.frame_previz(),
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match (&app.ndi, &app.ndi_error) {
                    (Some(ndi), _) => {
                        ui.label(RichText::new(format!("NDI {}", ndi.version())).weak());
                    }
                    (None, Some(_)) => {
                        // Not fatal — you can lay a rig out with no runtime — but
                        // it must be obvious why nothing is arriving.
                        ui.label(
                            RichText::new("no NDI runtime").color(Color32::from_rgb(220, 120, 60)),
                        );
                    }
                    _ => {}
                }
            });
        });
    });
}

pub fn status_bar(ui: &mut egui::Ui, app: &mut App, open_outputs: usize) {
    egui::containers::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            let problems = app.show.validate();
            let errors = problems
                .iter()
                .filter(|p| p.severity == Severity::Error)
                .count();
            let warnings = problems.len() - errors;

            if errors > 0 {
                ui.label(
                    RichText::new(format!("⛔ {errors} error(s)"))
                        .color(Color32::from_rgb(220, 80, 80)),
                )
                .on_hover_text(
                    problems
                        .iter()
                        .filter(|p| p.severity == Severity::Error)
                        .map(|p| p.message.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if warnings > 0 {
                ui.label(
                    RichText::new(format!("⚠ {warnings} warning(s)"))
                        .color(Color32::from_rgb(220, 170, 60)),
                )
                .on_hover_text(
                    problems
                        .iter()
                        .filter(|p| p.severity == Severity::Warning)
                        .map(|p| p.message.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if problems.is_empty() && !app.show.panels.is_empty() {
                ui.label(RichText::new("✔ no problems").weak());
            }

            ui.separator();
            ui.label(
                RichText::new(format!(
                    "{} panel(s) · {}×{} canvas",
                    app.show.panels.iter().filter(|p| p.enabled).count(),
                    app.show.virtual_raster.width,
                    app.show.virtual_raster.height
                ))
                .weak(),
            );

            let wanted = app
                .show
                .outputs
                .iter()
                .filter(|o| {
                    o.enabled
                        && matches!(
                            o.target,
                            OutputTarget::Display { .. } | OutputTarget::Ndi { .. }
                        )
                })
                .count();
            if wanted > 0 || open_outputs > 0 {
                ui.separator();
                // Showing both numbers matters: an output that failed to open is
                // otherwise invisible from the main window.
                let text = format!("{open_outputs}/{wanted} output(s) open");
                if open_outputs < wanted {
                    ui.label(
                        RichText::new(format!("⚠ {text}")).color(Color32::from_rgb(220, 170, 60)),
                    );
                } else {
                    ui.label(RichText::new(text).weak());
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(t) = app.toasts.last() {
                    let colour = if t.is_error {
                        Color32::from_rgb(230, 100, 100)
                    } else {
                        Color32::from_rgb(140, 200, 140)
                    };
                    ui.label(RichText::new(&t.text).color(colour));
                }
            });
        });
    });
}

pub fn sources_panel(ui: &mut egui::Ui, app: &mut App, actions: &mut Actions) {
    egui::containers::Panel::left("sources")
        .default_size(300.0)
        .show(ui, |ui| {
            ui.heading("Sources");

            if let Some(err) = app.ndi_error.clone() {
                ui.colored_label(Color32::from_rgb(220, 120, 60), err);
                ui.separator();
            }

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        app.ndi.is_some() && !app.discovering,
                        egui::Button::new("Scan network"),
                    )
                    .clicked()
                {
                    actions.discover = true;
                }
                if app.discovering {
                    ui.spinner();
                }
                ui.label(RichText::new(format!("{} found", app.discovered.len())).weak());
            });

            ui.separator();

            if app.show.sources.is_empty() {
                ui.label(RichText::new("Import a Resolume Advanced Output to get started.").weak());
                return;
            }

            let discovered = app.discovered.clone();
            let mut dirty = false;

            egui::ScrollArea::vertical().show(ui, |ui| {
                for i in 0..app.show.sources.len() {
                    let id = app.show.sources[i].id.clone();
                    let status = app.receiver(&id).map(|r| r.status());

                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let src = &mut app.show.sources[i];
                            if ui.checkbox(&mut src.enabled, "").changed() {
                                dirty = true;
                            }
                            ui.strong(src.name.clone());
                        });

                        // Which quad this source is sampled with. The single most
                        // consequential setting in the app, so it is stated in
                        // words rather than left implicit.
                        let space = match &app.show.sources[i].space {
                            unmapper_core::SourceSpace::Composition => {
                                "whole composition — slices sample their Input rect".to_string()
                            }
                            unmapper_core::SourceSpace::ScreenRaster { screen_id } => {
                                format!(
                                    "screen {screen_id} output — slices sample their Output rect"
                                )
                            }
                        };
                        ui.label(RichText::new(space).weak().small());

                        let current = match &app.show.sources[i].kind {
                            SourceKind::Ndi { name } => name.clone(),
                            SourceKind::TestPattern => String::new(),
                            SourceKind::Still { path } => path.display().to_string(),
                        };

                        egui::ComboBox::from_id_salt(format!("ndi-{id}"))
                            .width(ui.available_width() - 8.0)
                            .selected_text(if current.is_empty() {
                                "— not bound —".to_string()
                            } else {
                                current.clone()
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(current.is_empty(), "— not bound —")
                                    .clicked()
                                {
                                    app.show.sources[i].kind = SourceKind::TestPattern;
                                    dirty = true;
                                }
                                for found in &discovered {
                                    if ui
                                        .selectable_label(current == found.name, &found.name)
                                        .clicked()
                                    {
                                        app.show.sources[i].kind = SourceKind::Ndi {
                                            name: found.name.clone(),
                                        };
                                        dirty = true;
                                    }
                                }
                            });

                        match status {
                            Some(s) if s.frames > 0 => {
                                let expected = app.show.sources[i].expected;
                                let mismatch = expected
                                    .is_some_and(|e| e.width != s.width || e.height != s.height);
                                let text = format!(
                                    "{}×{} {} · {:.0} fps",
                                    s.width,
                                    s.height,
                                    s.format.clone().unwrap_or_default(),
                                    s.fps
                                );
                                if mismatch {
                                    let e = expected.unwrap();
                                    // A feed that is not the size the slice map
                                    // says will sample the wrong regions, and the
                                    // wall looks subtly wrong rather than broken.
                                    ui.colored_label(
                                        Color32::from_rgb(220, 170, 60),
                                        format!(
                                            "⚠ {text} — slice map expects {}×{}",
                                            e.width, e.height
                                        ),
                                    );
                                } else {
                                    ui.colored_label(
                                        Color32::from_rgb(140, 200, 140),
                                        format!("● {text}"),
                                    );
                                }
                            }
                            Some(s) => {
                                ui.colored_label(
                                    Color32::from_rgb(200, 160, 60),
                                    if s.connected {
                                        "connected, waiting for a frame".to_string()
                                    } else {
                                        s.last_error.unwrap_or_else(|| "connecting…".into())
                                    },
                                );
                            }
                            None => {
                                ui.label(RichText::new("not receiving").weak());
                            }
                        }
                    });
                }
            });

            if dirty {
                app.dirty = true;
            }

            ui.add_space(8.0);
            ui.separator();
            backdrop_section(ui, app, actions);

            ui.add_space(8.0);
            ui.separator();
            model_section(ui, app, actions);

            ui.add_space(8.0);
            ui.separator();
            outputs_section(ui, app, actions);
        });
}

/// The 2D mockup the panels are positioned against.
///
/// An editing aid, never content: the viewport renders it, and the canvas that
/// outputs crop from does not. Dragging a panel onto the right place in a render
/// of the set is the whole point of it.
fn backdrop_section(ui: &mut egui::Ui, app: &mut App, actions: &mut Actions) {
    ui.heading("Backdrop");

    let Some(backdrop) = &mut app.show.geometry.backdrop else {
        ui.label(
            RichText::new("A render, plan or photo of the set, to place panels against.")
                .weak()
                .small(),
        );
        if ui.button("Choose an image…").clicked() {
            actions.pick_backdrop = true;
        }
        return;
    };

    let mut dirty = false;
    let mut clear = false;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(
                backdrop
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| backdrop.path.display().to_string()),
            )
            .weak()
            .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("✖")
                .on_hover_text("Remove the backdrop")
                .clicked()
            {
                clear = true;
            }
            if ui.button("Change…").clicked() {
                actions.pick_backdrop = true;
            }
        });
    });

    ui.horizontal(|ui| {
        ui.label("Opacity");
        dirty |= ui
            .add(egui::Slider::new(&mut backdrop.opacity, 0.0..=1.0))
            .on_hover_text("Fade the mockup so panels stay readable over a busy render")
            .changed();
    });

    egui::Grid::new("backdrop-rect")
        .num_columns(4)
        .show(ui, |ui| {
            ui.label("X");
            dirty |= ui
                .add(egui::DragValue::new(&mut backdrop.rect.x).speed(1.0))
                .changed();
            ui.label("Y");
            dirty |= ui
                .add(egui::DragValue::new(&mut backdrop.rect.y).speed(1.0))
                .changed();
            ui.end_row();
            ui.label("W");
            dirty |= ui
                .add(
                    egui::DragValue::new(&mut backdrop.rect.width)
                        .speed(1.0)
                        .range(1.0..=65536.0),
                )
                .changed();
            ui.label("H");
            dirty |= ui
                .add(
                    egui::DragValue::new(&mut backdrop.rect.height)
                        .speed(1.0)
                        .range(1.0..=65536.0),
                )
                .changed();
            ui.end_row();
        });

    let raster = app.show.virtual_raster;
    if ui
        .button("Fit to canvas")
        .on_hover_text("Stretch the mockup across the whole virtual raster")
        .clicked()
    {
        if let Some(b) = &mut app.show.geometry.backdrop {
            b.rect = Rect::new(0.0, 0.0, raster.width as f32, raster.height as f32);
            dirty = true;
        }
    }

    if clear {
        app.show.geometry.backdrop = None;
        dirty = true;
    }
    if dirty {
        app.dirty = true;
    }
}

/// The set model shown behind the panels in the previz view.
fn model_section(ui: &mut egui::Ui, app: &mut App, actions: &mut Actions) {
    ui.heading("Set model");

    let Some(model) = &mut app.show.geometry.model else {
        ui.label(
            RichText::new("A glTF or GLB export of the set, for the Previz view.")
                .weak()
                .small(),
        );
        if ui.button("Choose a model…").clicked() {
            actions.pick_model = true;
        }
        return;
    };

    let mut dirty = false;
    let mut clear = false;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(
                model
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "(none)".into()),
            )
            .weak()
            .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖").on_hover_text("Remove the model").clicked() {
                clear = true;
            }
            if ui.button("Change…").clicked() {
                actions.pick_model = true;
            }
        });
    });

    ui.horizontal(|ui| {
        ui.label("Scale");
        dirty |= ui
            .add(
                egui::DragValue::new(&mut model.scale)
                    .speed(0.001)
                    .range(1e-4..=1000.0),
            )
            .on_hover_text(
                "CAD is often exported in millimetres; 0.001 turns those into the metres                  the stage uses",
            )
            .changed();
        // The unit mismatch is the single most common reason a model comes in
        // invisible or enormous, so the fix is one click rather than a guess.
        if ui.small_button("mm→m").clicked() {
            model.scale = 0.001;
            dirty = true;
        }
        if ui.small_button("1:1").clicked() {
            model.scale = 1.0;
            dirty = true;
        }
    });

    egui::Grid::new("model-pos").num_columns(2).show(ui, |ui| {
        ui.label("X");
        dirty |= ui
            .add(egui::DragValue::new(&mut model.translation.x).speed(0.01))
            .changed();
        ui.end_row();
        ui.label("Y");
        dirty |= ui
            .add(egui::DragValue::new(&mut model.translation.y).speed(0.01))
            .changed();
        ui.end_row();
        ui.label("Z");
        dirty |= ui
            .add(egui::DragValue::new(&mut model.translation.z).speed(0.01))
            .changed();
        ui.end_row();
    });

    let (mut yaw, _, _) = model.rotation.to_euler(glam::EulerRot::YXZ);
    let mut yaw_deg = yaw.to_degrees();
    ui.horizontal(|ui| {
        ui.label("Yaw°");
        if ui
            .add(
                egui::DragValue::new(&mut yaw_deg)
                    .speed(0.5)
                    .range(-180.0..=180.0),
            )
            .changed()
        {
            yaw = yaw_deg.to_radians();
            model.rotation = glam::Quat::from_rotation_y(yaw);
            dirty = true;
        }
    });

    if app.mode != ViewMode::Previz {
        ui.label(RichText::new("Switch to Previz to see it.").weak().small());
    }

    if clear {
        app.show.geometry.model = None;
        dirty = true;
    }
    if dirty {
        app.dirty = true;
    }
}

/// Where the rig is sent. Each output is one monitor standing in for a piece of
/// the wall, showing a crop of the emulation canvas.
fn outputs_section(ui: &mut egui::Ui, app: &mut App, actions: &mut Actions) {
    ui.horizontal(|ui| {
        ui.heading("Outputs");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Rescan")
                .on_hover_text("Look for connected displays again")
                .clicked()
            {
                actions.rescan_displays = true;
            }
        });
    });

    let monitors = app.monitors.clone();
    if monitors.is_empty() {
        ui.label(RichText::new("No displays reported yet.").weak());
    }

    let mut dirty = false;
    let mut remove: Option<usize> = None;

    for i in 0..app.show.outputs.len() {
        let id = app.show.outputs[i].id.clone();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                dirty |= ui.checkbox(&mut app.show.outputs[i].enabled, "").changed();
                dirty |= ui
                    .text_edit_singleline(&mut app.show.outputs[i].name)
                    .changed();
                if ui.button("✖").on_hover_text("Remove this output").clicked() {
                    remove = Some(i);
                }
            });

            // What kind of output. Switching keeps the region, since "the same
            // piece of wall, sent somewhere else" is the usual reason to change.
            let is_ndi = matches!(app.show.outputs[i].target, OutputTarget::Ndi { .. });
            ui.horizontal(|ui| {
                if ui.selectable_label(!is_ndi, "Display").clicked() && is_ndi {
                    app.show.outputs[i].target = OutputTarget::Display {
                        index: 0,
                        fullscreen: false,
                    };
                    dirty = true;
                }
                if ui.selectable_label(is_ndi, "NDI").clicked() && !is_ndi {
                    app.show.outputs[i].target = OutputTarget::Ndi {
                        name: format!("UnMapper {}", app.show.outputs[i].name),
                    };
                    dirty = true;
                }
            });

            if let OutputTarget::Ndi { name } = &mut app.show.outputs[i].target {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    dirty |= ui
                        .text_edit_singleline(name)
                        .on_hover_text("What this source is called on the network")
                        .changed();
                });
                ui.label(
                    RichText::new("Costs a GPU readback each frame.")
                        .weak()
                        .small(),
                );
            }

            // Which monitor.
            if let OutputTarget::Display { index, fullscreen } = &mut app.show.outputs[i].target {
                let selected = monitors
                    .get(*index)
                    .map(|m| m.label(*index))
                    .unwrap_or_else(|| format!("{index}: (not connected)"));
                egui::ComboBox::from_id_salt(format!("mon-{id}"))
                    .width(ui.available_width() - 8.0)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (m, info) in monitors.iter().enumerate() {
                            if ui.selectable_label(*index == m, info.label(m)).clicked() {
                                *index = m;
                                dirty = true;
                            }
                        }
                    });
                dirty |= ui.checkbox(fullscreen, "Fullscreen").changed();
            }

            // What this output shows. Previz outputs are rendered from the
            // camera rather than cropped, so they carry a size instead of a region.
            let is_previz = matches!(app.show.outputs[i].view, OutputView::Previz { .. });
            ui.horizontal(|ui| {
                if ui.selectable_label(!is_previz, "Emulation").clicked() && is_previz {
                    let size = app.show.outputs[i].size;
                    app.show.outputs[i].view = OutputView::Emulation {
                        region: Rect::new(0.0, 0.0, size.width as f32, size.height as f32),
                    };
                    dirty = true;
                }
                if ui.selectable_label(is_previz, "Previz").clicked() && !is_previz {
                    app.show.outputs[i].view = OutputView::Previz {
                        camera: Default::default(),
                    };
                    dirty = true;
                }
            });

            // Read the monitor before taking a mutable borrow of the view, since
            // both live in the same Output.
            let monitor_index = match &app.show.outputs[i].target {
                OutputTarget::Display { index, .. } => Some(*index),
                _ => None,
            };

            // Which piece of the canvas.
            if let OutputView::Emulation { region } = &mut app.show.outputs[i].view {
                egui::Grid::new(format!("region-{id}"))
                    .num_columns(4)
                    .show(ui, |ui| {
                        ui.label("X");
                        dirty |= ui
                            .add(egui::DragValue::new(&mut region.x).speed(1.0))
                            .changed();
                        ui.label("Y");
                        dirty |= ui
                            .add(egui::DragValue::new(&mut region.y).speed(1.0))
                            .changed();
                        ui.end_row();
                        ui.label("W");
                        dirty |= ui
                            .add(
                                egui::DragValue::new(&mut region.width)
                                    .speed(1.0)
                                    .range(1.0..=32768.0),
                            )
                            .changed();
                        ui.label("H");
                        dirty |= ui
                            .add(
                                egui::DragValue::new(&mut region.height)
                                    .speed(1.0)
                                    .range(1.0..=32768.0),
                            )
                            .changed();
                        ui.end_row();
                    });

                // One canvas pixel must be one screen pixel or the emulation is
                // not an emulation. Nearest sampling makes a mismatch visibly
                // blocky, but saying so is better than making them find out.
                if let Some(m) = monitor_index.and_then(|m| monitors.get(m)) {
                    let matches = region.width as u32 == m.size.width
                        && region.height as u32 == m.size.height;
                    if !matches {
                        ui.label(
                            RichText::new(format!(
                                "⚠ region is {}×{}, display is {}×{} — not 1:1",
                                region.width as u32,
                                region.height as u32,
                                m.size.width,
                                m.size.height
                            ))
                            .color(Color32::from_rgb(220, 170, 60))
                            .small(),
                        );
                        if ui.button("Match the display").clicked() {
                            region.width = m.size.width as f32;
                            region.height = m.size.height as f32;
                            dirty = true;
                        }
                    }
                }
            } else {
                ui.label(
                    RichText::new("Previz output — not yet rendered to a window.")
                        .weak()
                        .small(),
                );
            }
        });
    }

    if let Some(i) = remove {
        app.show.outputs.remove(i);
        dirty = true;
    }

    if ui.button("Add output").clicked() {
        app.show
            .outputs
            .push(new_display_output(&app.show, &monitors));
        dirty = true;
    }

    if dirty {
        app.dirty = true;
    }
}

/// A sensible new output: the next unused display, showing the top-left of the
/// canvas at that display's own size.
///
/// Deliberately **not** fullscreen. A fullscreen window that opens on the wrong
/// monitor, or before the region is set, is unpleasant to get rid of; ticking the
/// box is a decision the operator should make once things look right.
fn new_display_output(show: &Show, monitors: &[MonitorInfo]) -> Output {
    let used: Vec<usize> = show
        .outputs
        .iter()
        .filter_map(|o| match o.target {
            OutputTarget::Display { index, .. } => Some(index),
            _ => None,
        })
        .collect();
    let index = (0..monitors.len().max(1))
        .find(|i| !used.contains(i))
        .unwrap_or(0);

    let size = monitors
        .get(index)
        .map(|m| m.size)
        .unwrap_or(Size::new(1920, 1080));

    Output {
        id: format!("out-{}", show.outputs.len() + 1),
        name: monitors
            .get(index)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("Display {index}")),
        target: OutputTarget::Display {
            index,
            fullscreen: false,
        },
        view: OutputView::Emulation {
            region: Rect::new(0.0, 0.0, size.width as f32, size.height as f32),
        },
        size,
        enabled: true,
    }
}

pub fn inspector_panel(ui: &mut egui::Ui, app: &mut App) {
    egui::containers::Panel::right("inspector")
        .default_size(280.0)
        .show(ui, |ui| {
            ui.heading("Panel");

            let Some(id) = app.selected.clone() else {
                ui.label(RichText::new("Select a panel in the Emulation view.").weak());
                return;
            };
            let Some(index) = app.show.panels.iter().position(|p| p.id == id) else {
                app.select_panel(None);
                return;
            };

            let mut changed = false;
            let mut relayout = false;
            {
                let p = &mut app.show.panels[index];
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut p.enabled, "").changed();
                    changed |= ui.text_edit_singleline(&mut p.name).changed();
                });
                ui.label(RichText::new(&p.id).weak().small());

                ui.separator();
                ui.label(RichText::new("Position on the canvas (pixels)").strong());
                egui::Grid::new("layout").num_columns(2).show(ui, |ui| {
                    ui.label("X");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.layout.x).speed(1.0))
                        .changed();
                    ui.end_row();
                    ui.label("Y");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.layout.y).speed(1.0))
                        .changed();
                    ui.end_row();
                    ui.label("Width");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.layout.width)
                                .speed(1.0)
                                .range(1.0..=32768.0),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Height");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.layout.height)
                                .speed(1.0)
                                .range(1.0..=32768.0),
                        )
                        .changed();
                    ui.end_row();
                });

                ui.label(
                    RichText::new(format!(
                        "LED resolution {}×{}",
                        p.pixels.width, p.pixels.height
                    ))
                    .weak()
                    .small(),
                );

                ui.separator();
                ui.label(RichText::new("Position in the stage (metres)").strong());
                egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
                    ui.label("X");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.placement.translation.x).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Y");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.placement.translation.y).speed(0.01))
                        .changed();
                    ui.end_row();
                    ui.label("Z");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.placement.translation.z).speed(0.01))
                        .changed();
                    ui.end_row();
                });

                // Yaw is the one rotation a wall actually gets on a real stage,
                // and a quaternion is not something anyone should type.
                let (mut yaw, _, _) = p.placement.rotation.to_euler(glam::EulerRot::YXZ);
                let mut yaw_deg = yaw.to_degrees();
                ui.horizontal(|ui| {
                    ui.label("Yaw°");
                    if ui
                        .add(
                            egui::DragValue::new(&mut yaw_deg)
                                .speed(0.5)
                                .range(-180.0..=180.0),
                        )
                        .changed()
                    {
                        yaw = yaw_deg.to_radians();
                        p.placement.rotation = glam::Quat::from_rotation_y(yaw);
                        changed = true;
                    }
                });

                if let Some(pitch) = p.pitch_mm() {
                    ui.label(
                        RichText::new(format!("pixel pitch {pitch:.2} mm"))
                            .weak()
                            .small(),
                    );
                }
            }

            changed |= surface_section(ui, app, &id);

            ui.separator();
            if ui
                .button("Re-derive stage position from canvas layout")
                .on_hover_text(
                    "Lay every panel flat, at the position and scale its canvas layout implies",
                )
                .clicked()
            {
                relayout = true;
            }

            if relayout {
                app.show.arrange_panels_from_layout();
                changed = true;
            }
            if changed {
                app.dirty = true;
            }
        });
}

/// The surface designer: what shape this panel's LED surface is, and the
/// controls for that shape.
///
/// A panel is flat until someone says otherwise, and most stay that way — so
/// this section is the one place in the inspector that is usually a single row.
/// Everything below the kind picker belongs to the kind that is chosen.
fn surface_section(ui: &mut egui::Ui, app: &mut App, id: &str) -> bool {
    let Some(index) = app.panel_index(id) else {
        return false;
    };
    let mut changed = false;

    ui.separator();
    ui.label(RichText::new("Surface shape").strong());

    let kind = SurfaceKind::of(&app.show.panels[index].surface);
    let mut wanted = kind;
    ui.horizontal(|ui| {
        for k in [SurfaceKind::Flat, SurfaceKind::Arc, SurfaceKind::Lattice] {
            ui.selectable_value(&mut wanted, k, k.label());
        }
    });
    if wanted != kind {
        changed |= app.set_surface_kind(id, wanted);
    }

    match SurfaceKind::of(&app.show.panels[index].surface) {
        SurfaceKind::Flat => {
            ui.label(
                RichText::new("A rigid flat tile — what one physical panel is.")
                    .weak()
                    .small(),
            );
        }
        SurfaceKind::Arc => changed |= arc_controls(ui, app, index),
        SurfaceKind::Lattice => changed |= lattice_controls(ui, app, id, index),
    }

    changed
}

fn arc_controls(ui: &mut egui::Ui, app: &mut App, index: usize) -> bool {
    let size = app.show.panels[index].placement.size;
    let Surface::Arc { sweep_deg } = app.show.panels[index].surface else {
        return false;
    };

    let mut sweep = sweep_deg;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Sweep°");
        changed |= ui
            .add(
                egui::DragValue::new(&mut sweep)
                    .speed(0.5)
                    .range(-180.0..=180.0),
            )
            .on_hover_text("Positive sweeps both ends away from the audience")
            .changed();
    });

    // The sweep is the shape; these are the numbers that can be checked against
    // a drawing, which is how anyone finds out the sweep is wrong.
    match (Surface::Arc { sweep_deg: sweep }).arc_metrics(size) {
        Some(m) => {
            ui.label(
                RichText::new(format!(
                    "radius {:.2} m · chord {:.2} m · depth {:.2} m",
                    m.radius, m.chord, m.depth
                ))
                .weak()
                .small(),
            );
        }
        None => {
            ui.label(
                RichText::new("straight — the ends have not been swept yet")
                    .weak()
                    .small(),
            );
        }
    }

    if changed {
        app.show.panels[index].surface = Surface::Arc { sweep_deg: sweep };
        app.dirty = true;
    }
    changed
}

fn lattice_controls(ui: &mut egui::Ui, app: &mut App, id: &str, index: usize) -> bool {
    let Some((columns, rows)) = app.show.panels[index].surface.lattice_dims() else {
        return false;
    };
    let size = app.show.panels[index].placement.size;
    let mut changed = false;

    let (mut c, mut r) = (columns, rows);
    egui::Grid::new("lattice").num_columns(2).show(ui, |ui| {
        ui.label("Columns");
        ui.add(
            egui::DragValue::new(&mut c)
                .speed(0.1)
                .range(2..=MAX_LATTICE),
        );
        ui.end_row();
        ui.label("Rows");
        ui.add(
            egui::DragValue::new(&mut r)
                .speed(0.1)
                .range(2..=MAX_LATTICE),
        );
        ui.end_row();
    });
    if (c, r) != (columns, rows) {
        // Resampled, not rebuilt: changing the grid keeps the shape it already has.
        changed |= app.resize_lattice(id, c, r);
    }

    ui.label(
        RichText::new("Drag the points in the Previz view.")
            .weak()
            .small(),
    );

    let (columns, rows) = app.show.panels[index]
        .surface
        .lattice_dims()
        .unwrap_or((columns, rows));

    match app.selected_point {
        Some(i) if i < app.show.panels[index].surface.points().len() => {
            let local = app.show.panels[index].surface.points()[i];
            let (col, row) = (i as u32 % columns, i as u32 / columns);
            ui.label(
                RichText::new(format!("Point — column {}, row {}", col + 1, row + 1)).strong(),
            );

            let mut p = local;
            egui::Grid::new("surface point")
                .num_columns(2)
                .show(ui, |ui| {
                    for (axis, value) in [("X", &mut p.x), ("Y", &mut p.y), ("Z", &mut p.z)] {
                        ui.label(axis);
                        changed |= ui.add(egui::DragValue::new(value).speed(0.01)).changed();
                        ui.end_row();
                    }
                });
            ui.label(
                RichText::new("panel-local metres · +Z towards the audience")
                    .weak()
                    .small(),
            );

            if changed && p != local {
                app.show.panels[index].surface.set_point(i, p);
                app.dirty = true;
            }

            if ui.button("Reset this point").clicked() {
                let u = col as f32 / (columns - 1).max(1) as f32;
                let v = row as f32 / (rows - 1).max(1) as f32;
                let flat = Surface::Flat.local_point(u, v, size);
                app.show.panels[index].surface.set_point(i, flat);
                app.dirty = true;
                changed = true;
            }
        }
        _ => {
            // Including a stale index: the surface can be resampled from under a
            // selection made against the old grid.
            app.selected_point = None;
            ui.label(
                RichText::new("No point selected — click one in the Previz view.")
                    .weak()
                    .small(),
            );
        }
    }

    if ui
        .button("Flatten")
        .on_hover_text("Put every point back on the panel's plane, keeping the grid")
        .clicked()
    {
        if let Some(flat) = Surface::flat_lattice(size, columns, rows) {
            app.show.panels[index].surface = flat;
            app.dirty = true;
            changed = true;
        }
    }

    changed
}

/// Where each of the selected panel's control points lands on screen, in egui
/// points — what both the painter and the pointer are measured in.
fn screen_handles(
    panel: &Panel,
    camera: &Camera,
    aspect: f32,
    rect: egui::Rect,
) -> Vec<(usize, Vec2)> {
    handle_positions(panel, camera, aspect)
        .into_iter()
        .map(|(i, uv)| {
            (
                i,
                Vec2::new(
                    rect.left() + uv.x * rect.width(),
                    rect.top() + uv.y * rect.height(),
                ),
            )
        })
        .collect()
}

/// Draw the selected panel's surface over the previz image: the shape as a
/// wireframe, its control points as handles.
///
/// Deliberately depth-less. The overlay is painted flat over the finished image,
/// so a handle behind the set model still shows — hiding those would look more
/// correct and be unusable, because the point you most need to pull is routinely
/// the one tucked behind a truss.
fn paint_surface_overlay(
    painter: &egui::Painter,
    panel: &Panel,
    camera: &Camera,
    aspect: f32,
    rect: egui::Rect,
    handles: &[(usize, Vec2)],
    selected_point: Option<usize>,
) {
    let at = |u: f32, v: f32| {
        camera.project(panel.surface_point(u, v), aspect).map(|uv| {
            egui::pos2(
                rect.left() + uv.x * rect.width(),
                rect.top() + uv.y * rect.height(),
            )
        })
    };

    // The shape's own subdivision, so the wireframe is the geometry the renderer
    // draws rather than a smooth guess laid over a faceted panel. Capped: an arc
    // can ask for 128 segments and this is a hint, not a second render.
    let (cols, rows) = panel.subdivisions();
    let (cols, rows) = (cols.clamp(1, 64), rows.clamp(1, 64));
    let stroke = egui::Stroke::new(1.0, WIRE.gamma_multiply(0.7));

    let line = |a: Option<egui::Pos2>, b: Option<egui::Pos2>| {
        // A segment with an end behind the camera is dropped whole: interpolating
        // to the near plane is a lot of work to draw a line nobody can act on.
        if let (Some(a), Some(b)) = (a, b) {
            painter.line_segment([a, b], stroke);
        }
    };
    for r in 0..=rows {
        let v = r as f32 / rows as f32;
        for c in 0..cols {
            line(
                at(c as f32 / cols as f32, v),
                at((c + 1) as f32 / cols as f32, v),
            );
        }
    }
    for c in 0..=cols {
        let u = c as f32 / cols as f32;
        for r in 0..rows {
            line(
                at(u, r as f32 / rows as f32),
                at(u, (r + 1) as f32 / rows as f32),
            );
        }
    }

    for (i, pos) in handles {
        let pos = egui::pos2(pos.x, pos.y);
        let selected = selected_point == Some(*i);
        let radius = if selected { 6.0 } else { 4.0 };
        painter.circle_filled(pos, radius, if selected { HANDLE_SELECTED } else { HANDLE });
        // A dark ring, because a white dot on a white panel is not a handle.
        painter.circle_stroke(
            pos,
            radius,
            egui::Stroke::new(1.0, Color32::from_black_alpha(180)),
        );
    }
}

/// The central viewport. Returns the rect the render target should be drawn into.
pub fn viewport(
    ui: &mut egui::Ui,
    app: &mut App,
    texture: egui::TextureId,
    target_size: (u32, u32),
) -> egui::Rect {
    let mut painted = egui::Rect::NOTHING;

    egui::containers::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(Color32::from_gray(18)))
        .show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            painted = rect;

            // Publish the viewport size, then honour a pending frame request now
            // that there is something real to fit against.
            let ppp = ui.ctx().pixels_per_point();
            app.viewport_px = Vec2::new(rect.width() * ppp, rect.height() * ppp);
            if app.needs_frame && app.viewport_px.x > 1.0 {
                app.frame_canvas();
                app.frame_previz();
                app.needs_frame = false;
            }

            ui.painter().image(
                texture,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );

            match app.mode {
                ViewMode::Canvas => canvas_interaction(ui, app, rect, &response, target_size),
                ViewMode::Previz => previz_interaction(app, &response, ui, rect, target_size),
            }
        });

    painted
}

/// Screen point → canvas pixel.
fn to_canvas(app: &App, rect: egui::Rect, p: egui::Pos2) -> Vec2 {
    Vec2::new(
        (p.x - rect.left()) / app.zoom + app.pan.x,
        (p.y - rect.top()) / app.zoom + app.pan.y,
    )
}

fn canvas_interaction(
    ui: &egui::Ui,
    app: &mut App,
    rect: egui::Rect,
    response: &egui::Response,
    _target: (u32, u32),
) {
    // Zoom about the cursor, so the thing under the pointer stays under it.
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.01 {
            if let Some(pointer) = response.hover_pos() {
                let before = to_canvas(app, rect, pointer);
                app.zoom = (app.zoom * (1.0 + scroll * 0.002)).clamp(0.02, 8.0);
                let after = to_canvas(app, rect, pointer);
                app.pan += before - after;
            }
        }
    }

    if response.drag_started() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let canvas = to_canvas(app, rect, pointer);
            // Shift-drag pans even over a panel, so a dense rig is still navigable.
            let pan_modifier = ui.input(|i| i.modifiers.shift);
            match (pan_modifier, app.panel_at(canvas)) {
                (false, Some(id)) => {
                    let origin = app
                        .show
                        .panel(&id)
                        .map(|p| Vec2::new(p.layout.x, p.layout.y))
                        .unwrap_or(Vec2::ZERO);
                    app.select_panel(Some(id.clone()));
                    app.drag = Some(Drag::Panel {
                        id,
                        grab: canvas - origin,
                    });
                }
                _ => app.drag = Some(Drag::Pan),
            }
        }
    }

    if response.dragged() {
        match &app.drag {
            Some(Drag::Panel { id, grab }) => {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let (id, grab) = (id.clone(), *grab);
                    let canvas = to_canvas(app, rect, pointer);
                    app.move_panel(&id, canvas - grab);
                }
            }
            Some(Drag::Pan) => {
                let d = response.drag_delta();
                app.pan -= Vec2::new(d.x, d.y) / app.zoom;
            }
            // A surface handle belongs to the previz view; nothing to do here.
            _ => {}
        }
    }

    if response.drag_stopped() {
        app.drag = None;
    }

    // A plain click on empty canvas clears the selection.
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let canvas = to_canvas(app, rect, pointer);
            let hit = app.panel_at(canvas);
            app.select_panel(hit);
        }
    }
}

fn previz_interaction(
    app: &mut App,
    response: &egui::Response,
    ui: &egui::Ui,
    rect: egui::Rect,
    target: (u32, u32),
) {
    let camera = app.previz_camera();
    // The aspect the image was rendered at, not the rect's — they agree, and
    // reading it from the target is what keeps the overlay honest if they ever
    // stop agreeing again.
    let aspect = target.0.max(1) as f32 / target.1.max(1) as f32;

    let handles = match app.selected_panel() {
        Some(panel) => screen_handles(panel, &camera, aspect, rect),
        None => Vec::new(),
    };
    let uv_at = |p: egui::Pos2| {
        Vec2::new(
            (p.x - rect.left()) / rect.width().max(1.0),
            (p.y - rect.top()) / rect.height().max(1.0),
        )
    };

    if response.drag_started() {
        // Where the button went *down*, not where the pointer is now: by the
        // frame egui calls this a drag the pointer has already left the handle,
        // and hit-testing the current position picks nothing at all.
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        // A drag that starts on a handle pulls it; anywhere else orbits, so the
        // view stays navigable with a panel selected.
        let picked =
            origin.and_then(|p| nearest_handle(&handles, Vec2::new(p.x, p.y), HANDLE_PICK_RADIUS));
        match (picked, app.selected.clone(), origin) {
            (Some(index), Some(panel), Some(pointer)) => {
                // Grab where it was taken hold of, not by its centre: a handle
                // that jumps under the cursor on the first frame has already
                // moved the wall before the operator has done anything.
                let grab = app
                    .surface_handle(&panel, index)
                    .and_then(|handle| {
                        camera
                            .ray(uv_at(pointer), aspect)
                            .intersect_plane(handle, camera.forward())
                            .map(|hit| handle - hit)
                    })
                    .unwrap_or(Vec3::ZERO);
                app.selected_point = Some(index);
                app.drag = Some(Drag::SurfacePoint { panel, index, grab });
            }
            _ => app.drag = None,
        }
    }

    if response.dragged() {
        match &app.drag {
            Some(Drag::SurfacePoint { panel, index, grab }) => {
                let (panel, index, grab) = (panel.clone(), *index, *grab);
                if let (Some(pointer), Some(handle)) = (
                    response.interact_pointer_pos(),
                    app.surface_handle(&panel, index),
                ) {
                    // Drag in the plane through the handle that faces the camera:
                    // the one plane where the pointer and the handle move
                    // together at every angle, so nothing runs away at a
                    // glancing view.
                    if let Some(hit) = camera
                        .ray(uv_at(pointer), aspect)
                        .intersect_plane(handle, camera.forward())
                    {
                        app.set_surface_point(&panel, index, hit + grab);
                    }
                }
            }
            _ => {
                let d = response.drag_delta();
                app.orbit_yaw -= d.x * 0.01;
                // Stop just short of straight up: at exactly vertical the view
                // matrix's up vector becomes parallel to the view direction and
                // the image flips.
                const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.02;
                app.orbit_pitch = (app.orbit_pitch + d.y * 0.01).clamp(-LIMIT, LIMIT);
            }
        }
    }

    if response.drag_stopped() {
        app.drag = None;
    }

    // A click selects a handle, or clears the selection by missing every one.
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            app.selected_point = nearest_handle(
                &handles,
                Vec2::new(pointer.x, pointer.y),
                HANDLE_PICK_RADIUS,
            );
        }
    }

    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.01 {
            app.orbit_distance = (app.orbit_distance * (1.0 - scroll * 0.002)).clamp(0.5, 500.0);
        }
    }

    // Painted last, and after the image the viewport drew, so it lands on top.
    if let Some(panel) = app.selected_panel() {
        paint_surface_overlay(
            ui.painter(),
            panel,
            &camera,
            aspect,
            rect,
            &handles,
            app.selected_point,
        );
    }
}

/// Turn an imported slice map into a show, keeping placement work if one is
/// already open.
pub fn apply_import(app: &mut App, map: unmapper_core::SliceMap, path: PathBuf, pitch: f32) {
    let warnings = map.warnings.clone();

    if app.show.slice_map.is_some() && !app.show.panels.is_empty() {
        // Re-import onto the existing rig rather than throwing the layout away.
        let report = app.show.reapply_slice_map(map, pitch);
        app.dirty = true;
        app.toast(format!(
            "Re-imported: {} slice(s) updated, {} added, {} orphaned",
            report.updated, report.added, report.orphaned
        ));
    } else {
        let show = Show::from_slice_map(map, pitch);
        app.replace_show(show, None);
        app.dirty = true;
        app.toast(format!("Imported {}", path.display()));
    }

    for w in warnings {
        app.error(w);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use unmapper_core::Panel;

    /// The previz image's size in pixels, and — since the viewport is the only
    /// thing in these frames — the window's.
    const VIEW: (u32, u32) = (800, 600);

    /// Drive the widgets with no window, no GPU and no NDI.
    ///
    /// egui needs neither: a `Context` fed `RawInput` runs the same interaction
    /// code a real pointer does. This is the only way anything in this file gets
    /// *clicked* on this machine, and the drag it exercises — pick a control
    /// point, pull it, watch the surface change — is the whole feature.
    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::Rect {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(VIEW.0 as f32, VIEW.1 as f32),
            )),
            events,
            ..Default::default()
        };
        let mut painted = egui::Rect::NOTHING;
        let mut out = ctx.run_ui(input, |ui| {
            painted = viewport(ui, app, egui::TextureId::Managed(0), VIEW);
        });
        // There is no renderer here to consume them, and since epaint 0.36
        // dropping a TexturesDelta with unapplied deltas panics rather than
        // passing silently. Dropping them is the intent of a headless frame, so
        // say so the way epaint asks.
        out.textures_delta.clear();
        painted
    }

    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn release(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }]
    }

    /// One 2.6 x 1.3 m panel, face on, six metres away, its surface a lattice —
    /// a wall filling enough of the frame that its handles are metres apart.
    fn previz_app() -> App {
        let mut app = App::headless();
        app.show.panels.push(Panel::from_layout(
            "a",
            "A",
            Size::new(1000, 500),
            Rect::new(0.0, 0.0, 1000.0, 500.0),
            2.6,
        ));
        app.mode = ViewMode::Previz;
        app.select_panel(Some("a".into()));
        assert!(app.set_surface_kind("a", SurfaceKind::Lattice));
        app.orbit_yaw = 0.0;
        app.orbit_pitch = 0.0;
        app.orbit_distance = 6.0;
        app.dirty = false;
        app
    }

    /// Where a control point is on screen, by the same route the overlay draws it.
    fn handle_on_screen(app: &App, rect: egui::Rect, index: usize) -> egui::Pos2 {
        let camera = app.previz_camera();
        let aspect = VIEW.0 as f32 / VIEW.1 as f32;
        let handles = screen_handles(app.selected_panel().unwrap(), &camera, aspect, rect);
        let at = handles
            .iter()
            .find(|(i, _)| *i == index)
            .unwrap_or_else(|| panic!("point {index} is not on screen"))
            .1;
        egui::pos2(at.x, at.y)
    }

    #[test]
    fn dragging_a_control_point_in_previz_moves_the_surface_under_the_pointer() {
        let ctx = egui::Context::default();
        let mut app = previz_app();

        // A first pass to lay the viewport out, then find the middle handle.
        let rect = frame(&ctx, &mut app, Vec::new());
        const CENTRE: usize = 7; // 5 x 3 lattice, middle row, middle column.
        let start = handle_on_screen(&app, rect, CENTRE);
        let before = app.surface_handle("a", CENTRE).unwrap();

        // Grab it a couple of points off centre — the pick radius is generous,
        // and a handle that snaps itself under the cursor has already moved the
        // wall before the operator has done anything.
        let grabbed = start + egui::vec2(3.0, -2.0);
        frame(&ctx, &mut app, press(grabbed));
        let dragged_to = grabbed + egui::vec2(40.0, 20.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(dragged_to)]);

        let after = app.surface_handle("a", CENTRE).unwrap();
        assert_eq!(app.selected_point, Some(CENTRE));
        assert!(app.dirty, "dragging a point is an edit");

        // Screen right is +X and screen down is -Y for a camera looking along -Z.
        assert!(after.x > before.x + 0.05, "{before:?} -> {after:?}");
        assert!(after.y < before.y - 0.02, "{before:?} -> {after:?}");
        // The drag plane faces the camera, so depth is the one thing it must not
        // change: a point that wandered in Z would push the wall through the set.
        assert!((after.z - before.z).abs() < 1e-3, "{before:?} -> {after:?}");

        // It went where it was put, not merely somewhere: the handle ends up
        // under the pointer, offset by exactly the grab it was taken hold of by.
        frame(&ctx, &mut app, release(dragged_to));
        let landed = handle_on_screen(&app, rect, CENTRE);
        let wanted = start + (dragged_to - grabbed);
        assert!(
            (landed - wanted).length() < 2.0,
            "handle landed at {landed:?}, wanted {wanted:?}"
        );
        assert!(app.drag.is_none(), "the drag should end with the button");

        // And only that point moved.
        let corner = app.surface_handle("a", 0).unwrap();
        let flat = app.show.panel("a").unwrap().placement.corners()[0];
        assert!((corner - flat).length() < 1e-4, "the corner moved too");
    }

    #[test]
    fn dragging_off_a_handle_orbits_the_camera_and_leaves_the_surface_alone() {
        let ctx = egui::Context::default();
        let mut app = previz_app();
        let rect = frame(&ctx, &mut app, Vec::new());
        let before = app.show.panel("a").unwrap().surface.points().to_vec();
        let yaw = app.orbit_yaw;

        // The top-left corner of the viewport: a long way from any handle.
        let empty = rect.min + egui::vec2(12.0, 12.0);
        frame(&ctx, &mut app, press(empty));
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(empty + egui::vec2(60.0, 0.0))],
        );

        assert!(
            (app.orbit_yaw - yaw).abs() > 0.1,
            "the view should have orbited"
        );
        assert_eq!(app.show.panel("a").unwrap().surface.points(), before);
        assert!(!app.dirty, "orbiting is not an edit");
    }

    #[test]
    fn clicking_selects_a_handle_and_clicking_away_clears_it() {
        let ctx = egui::Context::default();
        let mut app = previz_app();
        let rect = frame(&ctx, &mut app, Vec::new());
        let at = handle_on_screen(&app, rect, 2);

        frame(&ctx, &mut app, press(at));
        frame(&ctx, &mut app, release(at));
        assert_eq!(app.selected_point, Some(2));

        let empty = rect.min + egui::vec2(12.0, 12.0);
        frame(&ctx, &mut app, press(empty));
        frame(&ctx, &mut app, release(empty));
        assert_eq!(app.selected_point, None);
        assert!(!app.dirty, "clicking about is not an edit");
    }

    #[test]
    fn a_flat_panel_has_no_handles_to_grab() {
        // Every rig in existence is flat, and the previz view has to stay an
        // orbit-and-look view for all of them.
        let ctx = egui::Context::default();
        let mut app = previz_app();
        assert!(app.set_surface_kind("a", SurfaceKind::Flat));
        app.dirty = false;
        let rect = frame(&ctx, &mut app, Vec::new());

        let middle = rect.center();
        frame(&ctx, &mut app, press(middle));
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(middle + egui::vec2(40.0, 0.0))],
        );
        assert_eq!(app.selected_point, None);
        assert!(!app.dirty);
        assert!(app.orbit_yaw.abs() > 0.1, "it should have orbited instead");
    }
}
