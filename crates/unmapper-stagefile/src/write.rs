//! Writing a [`Show`] as stage XML.

use std::io::Cursor;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use unmapper_core::{
    Camera, OutputTarget, OutputView, Panel, Quad, Rect, Show, Size, SourceKind, SourceSpace,
    Surface, Vec3, WarpMesh,
};

use crate::{RASTER_DECLARED, RASTER_FALLBACK, RASTER_SLICE_BOUNDS, STAGE_FORMAT};

type W = Writer<Cursor<Vec<u8>>>;

/// Format a float for the file.
///
/// Rust's `Display` for `f32` already prints the *shortest string that parses
/// back to the same bits*, and prints whole numbers without a decimal point — so
/// this is both maximally legible (`960`, not `960.0`) and exactly lossless, at
/// once. An earlier version of this rounded to 4 decimal places for tidiness and
/// perturbed panel rotations on every save; rounding buys nothing here.
fn num(v: f32) -> String {
    format!("{v}")
}

fn open<'a>(name: &'a str, attrs: &[(&str, String)]) -> BytesStart<'a> {
    let mut el = BytesStart::new(name);
    for (k, v) in attrs {
        el.push_attribute((*k, v.as_str()));
    }
    el
}

fn empty(w: &mut W, name: &str, attrs: &[(&str, String)]) -> std::io::Result<()> {
    w.write_event(Event::Empty(open(name, attrs)))
}

fn start(w: &mut W, name: &str, attrs: &[(&str, String)]) -> std::io::Result<()> {
    w.write_event(Event::Start(open(name, attrs)))
}

fn end(w: &mut W, name: &str) -> std::io::Result<()> {
    w.write_event(Event::End(BytesEnd::new(name)))
}

/// An element holding only text, e.g. `<Warning>…</Warning>`.
fn text_element(w: &mut W, name: &str, body: &str) -> std::io::Result<()> {
    start(w, name, &[])?;
    w.write_event(Event::Text(BytesText::new(body)))?;
    end(w, name)
}

fn size_attrs(size: Size) -> Vec<(&'static str, String)> {
    vec![
        ("width", size.width.to_string()),
        ("height", size.height.to_string()),
    ]
}

fn rect_attrs(r: Rect) -> Vec<(&'static str, String)> {
    vec![
        ("x", num(r.x)),
        ("y", num(r.y)),
        ("width", num(r.width)),
        ("height", num(r.height)),
    ]
}

fn vec3_attrs(v: Vec3) -> Vec<(&'static str, String)> {
    vec![("x", num(v.x)), ("y", num(v.y)), ("z", num(v.z))]
}

/// A quad as four `<v x= y=>` children.
///
/// Deliberately the same shape Resolume uses for its own `InputRect` and
/// `OutputRect`, and in the same corner order, so anyone who has read an
/// Advanced Output recognises it immediately.
fn write_quad(w: &mut W, name: &str, q: Quad) -> std::io::Result<()> {
    start(w, name, &[])?;
    for p in q.corners() {
        empty(w, "v", &[("x", num(p.x)), ("y", num(p.y))])?;
    }
    end(w, name)
}

/// A warp lattice as `columns * rows` `<v x= y=>` children, row-major.
///
/// Same shape and same order as Resolume's own `<BezierWarper><vertices>`, and
/// the mode is written with Resolume's own token, so a lattice read out of an
/// Advanced Output and written back here is recognisably the same thing.
fn write_mesh(w: &mut W, mesh: &WarpMesh) -> std::io::Result<()> {
    start(
        w,
        "WarpMesh",
        &[
            ("columns", mesh.columns.to_string()),
            ("rows", mesh.rows.to_string()),
            ("mode", mesh.mode.as_str().to_owned()),
        ],
    )?;
    for p in &mesh.points {
        empty(w, "v", &[("x", num(p.x)), ("y", num(p.y))])?;
    }
    end(w, "WarpMesh")
}

fn write_camera(w: &mut W, c: &Camera) -> std::io::Result<()> {
    start(
        w,
        "Camera",
        &[
            ("fovY", num(c.fov_y_deg)),
            ("near", num(c.near)),
            ("far", num(c.far)),
        ],
    )?;
    empty(w, "Position", &vec3_attrs(c.position))?;
    empty(w, "Target", &vec3_attrs(c.target))?;
    empty(w, "Up", &vec3_attrs(c.up))?;
    end(w, "Camera")
}

fn write_panel(w: &mut W, p: &Panel) -> std::io::Result<()> {
    start(
        w,
        "Panel",
        &[
            ("id", p.id.clone()),
            ("name", p.name.clone()),
            ("enabled", p.enabled.to_string()),
        ],
    )?;
    empty(w, "Pixels", &size_attrs(p.pixels))?;
    empty(w, "Layout", &rect_attrs(p.layout))?;

    start(w, "Placement", &[])?;
    empty(w, "Translation", &vec3_attrs(p.placement.translation))?;
    let r = p.placement.rotation;
    empty(
        w,
        "Rotation",
        &[
            ("x", num(r.x)),
            ("y", num(r.y)),
            ("z", num(r.z)),
            ("w", num(r.w)),
        ],
    )?;
    empty(
        w,
        "Size",
        &[
            ("width", num(p.placement.size.x)),
            ("height", num(p.placement.size.y)),
        ],
    )?;
    end(w, "Placement")?;

    // Flat is the default and by far the common case, so it is written as
    // nothing at all. Every stage file from before surfaces existed stays
    // byte-identical, and a diff only ever shows a surface someone chose.
    match &p.surface {
        Surface::Flat => {}
        Surface::Arc { sweep_deg } => empty(
            w,
            "Surface",
            &[("kind", "arc".into()), ("sweepDeg", num(*sweep_deg))],
        )?,
        Surface::Lattice {
            columns,
            rows,
            points,
        } => {
            start(
                w,
                "Surface",
                &[
                    ("kind", "lattice".into()),
                    ("columns", columns.to_string()),
                    ("rows", rows.to_string()),
                ],
            )?;
            for pt in points {
                empty(w, "v", &vec3_attrs(*pt))?;
            }
            end(w, "Surface")?;
        }
    }

    end(w, "Panel")
}

pub(crate) fn to_xml(show: &Show) -> std::io::Result<String> {
    let mut w: W = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    w.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        None,
    )))?;

    start(
        &mut w,
        "UnMapperStage",
        &[
            ("version", STAGE_FORMAT.to_string()),
            ("name", show.name.clone()),
        ],
    )?;

    empty(&mut w, "VirtualRaster", &size_attrs(show.virtual_raster))?;

    // Geometry — omitted entirely when there is none, rather than written as an
    // empty shell, so a hand-written stage file stays short.
    if show.geometry.backdrop.is_some() || show.geometry.model.is_some() {
        start(&mut w, "Geometry", &[])?;
        if let Some(b) = &show.geometry.backdrop {
            start(
                &mut w,
                "Backdrop",
                &[
                    ("path", b.path.display().to_string()),
                    ("opacity", num(b.opacity)),
                ],
            )?;
            empty(&mut w, "Rect", &rect_attrs(b.rect))?;
            end(&mut w, "Backdrop")?;
        }
        if let Some(m) = &show.geometry.model {
            start(
                &mut w,
                "Model",
                &[
                    ("path", m.path.display().to_string()),
                    ("scale", num(m.scale)),
                ],
            )?;
            empty(&mut w, "Translation", &vec3_attrs(m.translation))?;
            empty(
                &mut w,
                "Rotation",
                &[
                    ("x", num(m.rotation.x)),
                    ("y", num(m.rotation.y)),
                    ("z", num(m.rotation.z)),
                    ("w", num(m.rotation.w)),
                ],
            )?;
            end(&mut w, "Model")?;
        }
        end(&mut w, "Geometry")?;
    }

    start(&mut w, "Sources", &[])?;
    for s in &show.sources {
        start(
            &mut w,
            "Source",
            &[
                ("id", s.id.clone()),
                ("name", s.name.clone()),
                ("enabled", s.enabled.to_string()),
            ],
        )?;
        match &s.kind {
            SourceKind::Ndi { name } => empty(&mut w, "Ndi", &[("name", name.clone())])?,
            SourceKind::TestPattern => empty(&mut w, "TestPattern", &[])?,
            SourceKind::Still { path } => {
                empty(&mut w, "Still", &[("path", path.display().to_string())])?
            }
        }
        // Which quad samples this source — the single most important fact in the
        // file, so it is its own element rather than an attribute.
        match &s.space {
            SourceSpace::Composition => empty(&mut w, "Composition", &[])?,
            SourceSpace::ScreenRaster { screen_id } => {
                empty(&mut w, "ScreenRaster", &[("screen", screen_id.clone())])?
            }
        }
        if let Some(e) = s.expected {
            empty(&mut w, "Expected", &size_attrs(e))?;
        }
        end(&mut w, "Source")?;
    }
    end(&mut w, "Sources")?;

    start(&mut w, "Panels", &[])?;
    for p in &show.panels {
        write_panel(&mut w, p)?;
    }
    end(&mut w, "Panels")?;

    start(&mut w, "Bindings", &[])?;
    for b in &show.bindings {
        let mut attrs = vec![
            ("panel", b.panel_id.clone()),
            ("source", b.source_id.clone()),
        ];
        if let Some(s) = &b.slice_id {
            attrs.push(("slice", s.clone()));
        }
        start(&mut w, "Binding", &attrs)?;
        write_quad(&mut w, "SourceQuad", b.source_quad)?;
        if let Some(mesh) = &b.source_mesh {
            write_mesh(&mut w, mesh)?;
        }
        end(&mut w, "Binding")?;
    }
    end(&mut w, "Bindings")?;

    start(&mut w, "Outputs", &[])?;
    for o in &show.outputs {
        start(
            &mut w,
            "Output",
            &[
                ("id", o.id.clone()),
                ("name", o.name.clone()),
                ("enabled", o.enabled.to_string()),
            ],
        )?;
        match &o.target {
            OutputTarget::Display { index, fullscreen } => empty(
                &mut w,
                "Display",
                &[
                    ("index", index.to_string()),
                    ("fullscreen", fullscreen.to_string()),
                ],
            )?,
            OutputTarget::Ndi { name } => empty(&mut w, "Ndi", &[("name", name.clone())])?,
            OutputTarget::Syphon { name } => empty(&mut w, "Syphon", &[("name", name.clone())])?,
            OutputTarget::Spout { name } => empty(&mut w, "Spout", &[("name", name.clone())])?,
        }
        match &o.view {
            OutputView::Emulation { region } => empty(&mut w, "Emulation", &rect_attrs(*region))?,
            OutputView::Previz { camera } => {
                start(&mut w, "Previz", &[])?;
                write_camera(&mut w, camera)?;
                end(&mut w, "Previz")?;
            }
        }
        empty(&mut w, "Size", &size_attrs(o.size))?;
        end(&mut w, "Output")?;
    }
    end(&mut w, "Outputs")?;

    // The imported slice map is kept in full. It is provenance rather than
    // configuration — the bindings already carry the geometry that renders — but
    // keeping it means a re-import can be diffed against what was imported
    // before, and it costs a few lines per slice.
    if let Some(map) = &show.slice_map {
        let mut attrs = vec![("project", map.project_name.clone())];
        if let Some(c) = map.composition {
            attrs.push(("compositionWidth", c.width.to_string()));
            attrs.push(("compositionHeight", c.height.to_string()));
        }
        start(&mut w, "SliceMap", &attrs)?;
        for warning in &map.warnings {
            text_element(&mut w, "Warning", warning)?;
        }
        for screen in &map.screens {
            let mut sattrs = vec![
                ("id", screen.id.clone()),
                ("name", screen.name.clone()),
                (
                    "rasterSource",
                    match screen.raster_source {
                        unmapper_core::RasterSource::Declared => RASTER_DECLARED,
                        unmapper_core::RasterSource::SliceBounds => RASTER_SLICE_BOUNDS,
                        unmapper_core::RasterSource::Fallback => RASTER_FALLBACK,
                    }
                    .to_string(),
                ),
            ];
            if let Some(d) = &screen.device {
                sattrs.push(("device", d.clone()));
            }
            start(&mut w, "Screen", &sattrs)?;
            empty(&mut w, "Raster", &size_attrs(screen.raster))?;
            for note in &screen.notes {
                text_element(&mut w, "Note", note)?;
            }
            for slice in &screen.slices {
                start(
                    &mut w,
                    "Slice",
                    &[
                        ("id", slice.id.clone()),
                        ("name", slice.name.clone()),
                        ("enabled", slice.enabled.to_string()),
                        ("orientation", slice.orientation.to_string()),
                    ],
                )?;
                write_quad(&mut w, "InputRect", slice.input)?;
                write_quad(&mut w, "OutputRect", slice.output)?;
                if let Some(mesh) = &slice.warp {
                    write_mesh(&mut w, mesh)?;
                }
                end(&mut w, "Slice")?;
            }
            end(&mut w, "Screen")?;
        }
        end(&mut w, "SliceMap")?;
    }

    end(&mut w, "UnMapperStage")?;

    let mut out =
        String::from_utf8(w.into_inner().into_inner()).expect("quick-xml only ever writes UTF-8");
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_lose_their_pointless_decimal() {
        assert_eq!(num(960.0), "960");
        assert_eq!(num(-1.0), "-1");
        assert_eq!(num(0.0), "0");
    }

    #[test]
    fn fractions_round_trip_exactly() {
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(1.25), "1.25");
        // Every value must parse back to the identical bits, or a rotation
        // drifts a little further every time the stage is saved.
        for v in [0.5f32, 341.333_34, 0.198_669_33, -2.6, 1e-7, 0.001] {
            assert_eq!(num(v).parse::<f32>().unwrap(), v, "{v} did not round trip");
        }
    }
}
