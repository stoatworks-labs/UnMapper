//! The widgets.

use std::path::PathBuf;

use egui::{Color32, RichText};
use unmapper_core::{Severity, Show, SourceKind, Vec2};

use crate::state::{App, Drag, ViewMode};

/// What the UI is asking the host to do, when it cannot do it itself.
#[derive(Default)]
pub struct Actions {
    pub import_resolume: bool,
    pub open_stage: bool,
    pub save: bool,
    pub save_as: bool,
    pub discover: bool,
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

pub fn status_bar(ui: &mut egui::Ui, app: &mut App) {
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
        });
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
                app.selected = None;
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
                ViewMode::Previz => previz_interaction(app, &response, ui),
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
                    app.selected = Some(id.clone());
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
            None => {}
        }
    }

    if response.drag_stopped() {
        app.drag = None;
    }

    // A plain click on empty canvas clears the selection.
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let canvas = to_canvas(app, rect, pointer);
            app.selected = app.panel_at(canvas);
        }
    }
}

fn previz_interaction(app: &mut App, response: &egui::Response, ui: &egui::Ui) {
    if response.dragged() {
        let d = response.drag_delta();
        app.orbit_yaw -= d.x * 0.01;
        // Stop just short of straight up: at exactly vertical the view matrix's
        // up vector becomes parallel to the view direction and the image flips.
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.02;
        app.orbit_pitch = (app.orbit_pitch + d.y * 0.01).clamp(-LIMIT, LIMIT);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.01 {
            app.orbit_distance = (app.orbit_distance * (1.0 - scroll * 0.002)).clamp(0.5, 500.0);
        }
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
