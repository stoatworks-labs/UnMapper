//! Stoatworks Labs - About window for the egui apps.
//!
//! The same six things every other Stoatworks Labs product shows: the name, the
//! version it is actually running, its user guide, its project page, its source,
//! and the four ways to fund the work - over the Stoatworks Labs mark.
//!
//! This file is the MASTER, in stoatworks-backend/about/rust. It is vendored
//! into each egui repo by ../../scripts/sync-about.py - edit it THERE and re-run
//! the sync, never the copies. The facts come from `about_data.rs` beside it,
//! which is generated from the website's projects.json.
//!
//! # Using it
//!
//! Keep one flag in the app state and call the window every frame:
//!
//! ```ignore
//! // in the menu bar
//! if ui.button("About").clicked() {
//!     app.show_about = true;
//!     ui.close();
//! }
//!
//! // once per frame, after the panels
//! about_window::show(ctx, &mut app.show_about);
//! ```
//!
//! # The version
//!
//! `CARGO_PKG_VERSION` - the version cargo actually built, never a copy. The
//! `VERSION_FALLBACK` in about_data.rs is only for a caller that has no crate
//! version of its own to offer.

use crate::about_data as data;

/// The mark, decoded once and kept for the life of the process.
///
/// Carried as bytes in the binary rather than loaded from disk: these ship as a
/// single executable, and a sibling asset is one path to get wrong per platform.
const MARK_PNG: &[u8] = include_bytes!("about_mark.png");

/// Draw the About window if `open` is true. Returns nothing; `open` is cleared
/// when the user closes it, which is how egui models a window's lifetime.
pub fn show(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }

    egui::Window::new(format!("About {}", data::NAME))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| contents(ui));
}

fn contents(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    draw_mark(ui, rect);

    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.heading(data::NAME);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .monospace()
                    .color(egui::Color32::from_rgb(0x4c, 0xc9, 0xf0)),
            );
            if !data::LICENCE.is_empty() {
                ui.label(egui::RichText::new(format!("{} licensed", data::LICENCE)).weak());
            }
        });

        if !data::HOOK.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(data::HOOK).weak());
        }

        // A link is shown only if this product actually has one: a guide that
        // has not been written, or a repo that is still private, is an empty
        // string here and is left out rather than pointed at a URL that 404s.
        let rows = [
            ("User guide", data::GUIDE),
            ("Project page", data::PAGE),
            ("Source on GitHub", data::REPO),
        ];
        if rows.iter().any(|(_, url)| !url.is_empty()) {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("DOCUMENTATION").small().weak());
            for (label, url) in rows {
                if !url.is_empty() {
                    ui.hyperlink_to(label, url);
                }
            }
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new("SUPPORT THE WORK").small().weak());
        ui.horizontal_wrapped(|ui| {
            for (name, url) in data::FUNDING {
                ui.hyperlink_to(name, url);
            }
        });

        ui.add_space(12.0);
        ui.separator();
        ui.label(
            egui::RichText::new(format!("{} - {}", data::ORG, data::TAGLINE))
                .small()
                .weak(),
        );
        ui.hyperlink_to(data::HOME, data::HOME);
    });
}

/// Paint the mark behind the text, faintly, centred on the window.
///
/// egui has no z-order within a `Ui`, so this is drawn into the background
/// layer of the painter before the widgets go down. The tint carries the
/// opacity: egui images take a colour multiplier, not an alpha channel.
fn draw_mark(ui: &mut egui::Ui, rect: egui::Rect) {
    let texture = ui.ctx().memory_mut(|memory| {
        memory
            .data
            .get_temp::<egui::TextureHandle>(egui::Id::new("stoatworks-about-mark"))
    });

    let texture = match texture {
        Some(texture) => texture,
        None => {
            let image = match image_from_png(MARK_PNG) {
                Some(image) => image,
                // A mark that will not decode is not worth failing an About
                // window over.
                None => return,
            };
            let handle = ui.ctx().load_texture(
                "stoatworks-about-mark",
                image,
                egui::TextureOptions::LINEAR,
            );
            ui.ctx().memory_mut(|memory| {
                memory
                    .data
                    .insert_temp(egui::Id::new("stoatworks-about-mark"), handle.clone())
            });
            handle
        }
    };

    let size = texture.size_vec2();
    let width = rect.width() * 0.78;
    let scaled = egui::vec2(width, width * size.y / size.x);
    let where_to = egui::Rect::from_center_size(rect.center(), scaled);

    ui.painter().image(
        texture.id(),
        where_to,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::from_white_alpha(18),
    );
}

/// Decode the mark with the `image` crate, which every repo this lands in
/// already depends on.
fn image_from_png(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        decoded.as_raw(),
    ))
}
