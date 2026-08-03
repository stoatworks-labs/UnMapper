//! Reading stage XML back into a [`Show`].
//!
//! Deliberately more forgiving than the writer: unknown elements are ignored and
//! anything with a sensible default may be omitted. A stage file is something an
//! operator can reasonably hand-edit, and a reader that rejects a file for a
//! missing `enabled="true"` would make that miserable.
//!
//! What is *not* forgiven is a wrong shape — a binding naming a panel that is not
//! there, or a quad with three corners — because those produce a stage that looks
//! loaded and renders wrongly.

use roxmltree::{Document, Node};
use unmapper_core::{
    Backdrop, Binding, Camera, Model3d, Output, OutputTarget, OutputView, Panel, Placement3d, Quad,
    RasterSource, Rect, Screen, Show, Size, Slice, SliceMap, Source, SourceKind, SourceSpace,
    WarpMesh, WarpMode,
    StageGeometry, Vec2, Vec3, SHOW_FORMAT,
};

use crate::{StageError, RASTER_DECLARED, RASTER_FALLBACK, RASTER_SLICE_BOUNDS, STAGE_FORMAT};

fn attr<'a>(n: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    n.attribute(name)
}

fn f32_attr(n: Node, name: &str, default: f32) -> f32 {
    n.attribute(name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn u32_attr(n: Node, name: &str, default: u32) -> u32 {
    n.attribute(name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// XML booleans, accepting both `true`/`false` and Resolume's `1`/`0`.
fn bool_attr(n: Node, name: &str, default: bool) -> bool {
    match n.attribute(name) {
        Some(v) => matches!(v.trim(), "true" | "True" | "TRUE" | "1" | "yes"),
        None => default,
    }
}

fn child<'a>(n: Node<'a, 'a>, tag: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn children<'a>(n: Node<'a, 'a>, tag: &'a str) -> impl Iterator<Item = Node<'a, 'a>> + 'a {
    n.children().filter(move |c| c.has_tag_name(tag))
}

fn text_of(n: Node) -> String {
    n.text().unwrap_or_default().trim().to_owned()
}

fn size_of(n: Option<Node>, default: Size) -> Size {
    match n {
        Some(n) => Size::new(
            u32_attr(n, "width", default.width),
            u32_attr(n, "height", default.height),
        ),
        None => default,
    }
}

fn rect_of(n: Option<Node>) -> Rect {
    match n {
        Some(n) => Rect::new(
            f32_attr(n, "x", 0.0),
            f32_attr(n, "y", 0.0),
            f32_attr(n, "width", 0.0),
            f32_attr(n, "height", 0.0),
        ),
        None => Rect::from_size(0.0, 0.0),
    }
}

fn vec3_of(n: Option<Node>, default: Vec3) -> Vec3 {
    match n {
        Some(n) => Vec3::new(
            f32_attr(n, "x", default.x),
            f32_attr(n, "y", default.y),
            f32_attr(n, "z", default.z),
        ),
        None => default,
    }
}

fn quat_of(n: Option<Node>) -> glam::Quat {
    match n {
        Some(n) => {
            let q = glam::Quat::from_xyzw(
                f32_attr(n, "x", 0.0),
                f32_attr(n, "y", 0.0),
                f32_attr(n, "z", 0.0),
                f32_attr(n, "w", 1.0),
            );
            // A hand-edited rotation is very unlikely to be exactly unit length,
            // and a non-unit quaternion silently scales the panel it rotates.
            if q.length_squared() > 1e-6 {
                q.normalize()
            } else {
                glam::Quat::IDENTITY
            }
        }
        None => glam::Quat::IDENTITY,
    }
}

/// Four `<v x= y=>` children, in the writer's corner order.
fn quad_of(n: Option<Node>, what: &str) -> Result<Quad, StageError> {
    let Some(n) = n else {
        return Err(StageError::Malformed(format!("missing <{what}>")));
    };
    let pts: Vec<Vec2> = children(n, "v")
        .map(|v| Vec2::new(f32_attr(v, "x", 0.0), f32_attr(v, "y", 0.0)))
        .collect();
    if pts.len() != 4 {
        return Err(StageError::Malformed(format!(
            "<{what}> needs exactly 4 <v> corners, found {}",
            pts.len()
        )));
    }
    Ok(Quad::new(pts[0], pts[1], pts[2], pts[3]))
}

/// A `<WarpMesh>`, or `None` when the element is absent.
///
/// A malformed lattice is an error rather than a silent `None`: the alternative
/// is a stage that loads looking fine and renders a warped slice flat, which is
/// the sort of wrong that is only noticed in a venue.
fn mesh_of(n: Option<Node>) -> Result<Option<WarpMesh>, StageError> {
    let Some(n) = n else { return Ok(None) };
    let columns: u32 = n
        .attribute("columns")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let rows: u32 = n.attribute("rows").and_then(|v| v.parse().ok()).unwrap_or(0);
    let points: Vec<Vec2> = children(n, "v")
        .map(|v| Vec2::new(f32_attr(v, "x", 0.0), f32_attr(v, "y", 0.0)))
        .collect();
    let mode = n
        .attribute("mode")
        .map(WarpMode::from_param)
        .unwrap_or(WarpMode::Linear);

    let found = points.len();
    WarpMesh::new(columns, rows, points, mode)
        .map(Some)
        .ok_or_else(|| {
            StageError::Malformed(format!(
                "<WarpMesh> says {columns}x{rows} but carries {found} points, \
                 and both must be at least 2"
            ))
        })
}

fn camera_of(n: Option<Node>) -> Camera {
    let default = Camera::default();
    let Some(n) = n else { return default };
    Camera {
        position: vec3_of(child(n, "Position"), default.position),
        target: vec3_of(child(n, "Target"), default.target),
        up: vec3_of(child(n, "Up"), default.up),
        fov_y_deg: f32_attr(n, "fovY", default.fov_y_deg),
        near: f32_attr(n, "near", default.near),
        far: f32_attr(n, "far", default.far),
    }
}

pub(crate) fn from_xml(text: &str) -> Result<Show, StageError> {
    let doc = Document::parse(text)?;
    let root = doc.root_element();

    if !root.has_tag_name("UnMapperStage") {
        return Err(StageError::NotAStage {
            found: root.tag_name().name().to_owned(),
        });
    }

    let version = u32_attr(root, "version", 1);
    if version > STAGE_FORMAT {
        return Err(StageError::TooNew {
            found: version,
            supported: STAGE_FORMAT,
        });
    }

    let mut show = Show {
        format: SHOW_FORMAT,
        name: attr(root, "name").unwrap_or("Untitled").to_owned(),
        virtual_raster: size_of(child(root, "VirtualRaster"), Size::new(1920, 1080)),
        ..Default::default()
    };

    if let Some(g) = child(root, "Geometry") {
        let backdrop = child(g, "Backdrop").map(|b| Backdrop {
            path: attr(b, "path").unwrap_or_default().into(),
            rect: rect_of(child(b, "Rect")),
            opacity: f32_attr(b, "opacity", 1.0),
        });
        let model = child(g, "Model").map(|m| Model3d {
            path: attr(m, "path").unwrap_or_default().into(),
            scale: f32_attr(m, "scale", 1.0),
            rotation: quat_of(child(m, "Rotation")),
            translation: vec3_of(child(m, "Translation"), Vec3::ZERO),
        });
        show.geometry = StageGeometry { backdrop, model };
    }

    if let Some(sources) = child(root, "Sources") {
        for s in children(sources, "Source") {
            let kind = if let Some(n) = child(s, "Ndi") {
                SourceKind::Ndi {
                    name: attr(n, "name").unwrap_or_default().to_owned(),
                }
            } else if let Some(n) = child(s, "Still") {
                SourceKind::Still {
                    path: attr(n, "path").unwrap_or_default().into(),
                }
            } else {
                SourceKind::TestPattern
            };

            let space = match child(s, "ScreenRaster") {
                Some(n) => SourceSpace::ScreenRaster {
                    screen_id: attr(n, "screen").unwrap_or_default().to_owned(),
                },
                None => SourceSpace::Composition,
            };

            show.sources.push(Source {
                id: attr(s, "id").unwrap_or_default().to_owned(),
                name: attr(s, "name").unwrap_or_default().to_owned(),
                kind,
                space,
                expected: child(s, "Expected").map(|e| size_of(Some(e), Size::new(0, 0))),
                enabled: bool_attr(s, "enabled", true),
            });
        }
    }

    if let Some(panels) = child(root, "Panels") {
        for p in children(panels, "Panel") {
            let placement = match child(p, "Placement") {
                Some(pl) => {
                    let size = child(pl, "Size")
                        .map(|s| Vec2::new(f32_attr(s, "width", 1.0), f32_attr(s, "height", 1.0)))
                        .unwrap_or(Vec2::ONE);
                    Placement3d {
                        translation: vec3_of(child(pl, "Translation"), Vec3::ZERO),
                        rotation: quat_of(child(pl, "Rotation")),
                        size,
                    }
                }
                None => Placement3d::upright(Vec2::ONE),
            };

            show.panels.push(Panel {
                id: attr(p, "id").unwrap_or_default().to_owned(),
                name: attr(p, "name").unwrap_or_default().to_owned(),
                pixels: size_of(child(p, "Pixels"), Size::new(1, 1)),
                layout: rect_of(child(p, "Layout")),
                placement,
                enabled: bool_attr(p, "enabled", true),
            });
        }
    }

    if let Some(bindings) = child(root, "Bindings") {
        for b in children(bindings, "Binding") {
            show.bindings.push(Binding {
                panel_id: attr(b, "panel").unwrap_or_default().to_owned(),
                source_id: attr(b, "source").unwrap_or_default().to_owned(),
                source_quad: quad_of(child(b, "SourceQuad"), "SourceQuad")?,
                source_mesh: mesh_of(child(b, "WarpMesh"))?,
                slice_id: attr(b, "slice").map(str::to_owned),
            });
        }
    }

    if let Some(outputs) = child(root, "Outputs") {
        for o in children(outputs, "Output") {
            let target = if let Some(n) = child(o, "Display") {
                OutputTarget::Display {
                    index: n
                        .attribute("index")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    fullscreen: bool_attr(n, "fullscreen", true),
                }
            } else if let Some(n) = child(o, "Ndi") {
                OutputTarget::Ndi {
                    name: attr(n, "name").unwrap_or_default().to_owned(),
                }
            } else if let Some(n) = child(o, "Syphon") {
                OutputTarget::Syphon {
                    name: attr(n, "name").unwrap_or_default().to_owned(),
                }
            } else if let Some(n) = child(o, "Spout") {
                OutputTarget::Spout {
                    name: attr(n, "name").unwrap_or_default().to_owned(),
                }
            } else {
                return Err(StageError::Malformed(format!(
                    "output {:?} names no target (expected one of \
                     <Display>, <Ndi>, <Syphon>, <Spout>)",
                    attr(o, "name").unwrap_or("?")
                )));
            };

            let view = match child(o, "Previz") {
                Some(p) => OutputView::Previz {
                    camera: camera_of(child(p, "Camera")),
                },
                None => OutputView::Emulation {
                    region: rect_of(child(o, "Emulation")),
                },
            };

            show.outputs.push(Output {
                id: attr(o, "id").unwrap_or_default().to_owned(),
                name: attr(o, "name").unwrap_or_default().to_owned(),
                target,
                view,
                size: size_of(child(o, "Size"), Size::new(1920, 1080)),
                enabled: bool_attr(o, "enabled", true),
            });
        }
    }

    if let Some(m) = child(root, "SliceMap") {
        let composition = match (
            m.attribute("compositionWidth").and_then(|v| v.parse().ok()),
            m.attribute("compositionHeight")
                .and_then(|v| v.parse().ok()),
        ) {
            (Some(w), Some(h)) => Some(Size::new(w, h)),
            _ => None,
        };

        let mut screens = Vec::new();
        for s in children(m, "Screen") {
            let mut slices = Vec::new();
            for sl in children(s, "Slice") {
                slices.push(Slice {
                    id: attr(sl, "id").unwrap_or_default().to_owned(),
                    name: attr(sl, "name").unwrap_or_default().to_owned(),
                    input: quad_of(child(sl, "InputRect"), "InputRect")?,
                    output: quad_of(child(sl, "OutputRect"), "OutputRect")?,
                    enabled: bool_attr(sl, "enabled", true),
                    orientation: sl
                        .attribute("orientation")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    warp: mesh_of(child(sl, "WarpMesh"))?,
                });
            }
            screens.push(Screen {
                id: attr(s, "id").unwrap_or_default().to_owned(),
                name: attr(s, "name").unwrap_or_default().to_owned(),
                raster: size_of(child(s, "Raster"), Size::new(1920, 1080)),
                raster_source: match attr(s, "rasterSource") {
                    Some(RASTER_DECLARED) => RasterSource::Declared,
                    Some(RASTER_SLICE_BOUNDS) => RasterSource::SliceBounds,
                    Some(RASTER_FALLBACK) => RasterSource::Fallback,
                    // An unrecognised value must not silently become "trustworthy".
                    _ => RasterSource::Fallback,
                },
                device: attr(s, "device").map(str::to_owned),
                slices,
                notes: children(s, "Note").map(text_of).collect(),
            });
        }

        show.slice_map = Some(SliceMap {
            project_name: attr(m, "project").unwrap_or("imported").to_owned(),
            composition,
            screens,
            warnings: children(m, "Warning").map(text_of).collect(),
        });
    }

    // Structural checks the renderer would otherwise hit as a silently missing
    // panel. Cheap here, confusing later.
    for b in &show.bindings {
        if show.panel(&b.panel_id).is_none() {
            return Err(StageError::Malformed(format!(
                "binding names panel {:?}, which this stage does not contain",
                b.panel_id
            )));
        }
        if show.source(&b.source_id).is_none() {
            return Err(StageError::Malformed(format!(
                "binding names source {:?}, which this stage does not contain",
                b.source_id
            )));
        }
    }

    Ok(show)
}
