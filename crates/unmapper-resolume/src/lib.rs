//! Reads Resolume Arena's **Advanced Output** into a [`SliceMap`].
//!
//! # Schema provenance
//!
//! The element and attribute names handled here were read off real files written
//! by Resolume Arena 7.27.0, in two flavours:
//!
//! - `~/Documents/Resolume Arena/Preferences/AdvancedOutput.xml` — root `<ScreenSetup>`
//! - `~/Documents/Resolume Arena/Presets/Advanced Output/*.xml` — root `<XmlState>`
//!
//! They are not guessed from documentation. The same reverse-engineering backs the
//! exporters in the sibling projects `blend-calc` and `pixel-peeker`, and the reader
//! in `test-card`, so this one importer covers every Resolume-shaped file the fleet
//! produces or consumes. All four are in `tests/fixtures/`.
//!
//! # What this does differently from `test-card`'s reader
//!
//! `test-card` collapses each slice to a bounding box, which is correct for its
//! purpose — it draws outlines. UnMapper **samples textures** with these
//! coordinates, so a rotated or corner-pinned slice collapsed to its bounding box
//! would read the wrong texels. This reader keeps all four corners
//! ([`unmapper_core::Quad`]) and only ever uses the bounding box where a bounding box
//! is genuinely what is wanted.
//!
//! # What is deliberately not read
//!
//! Each slice carries a `<Warper>` holding a `BezierWarper` control grid and a
//! `Homography`. Those describe soft-edge and mesh warping applied *after* the
//! corner positions, and UnMapper does not reproduce them — a warped slice is
//! rendered as its four corners. This is a real limitation, not an oversight, and
//! it is reported through [`SliceMap::warnings`] rather than passed over quietly.

use roxmltree::{Document, Node};
use unmapper_core::{
    geom::{Quad, Rect, Vec2},
    RasterSource, Screen, Size, Slice, SliceMap,
};

/// Tolerance in pixels for calling a quad "not rotated". Resolume writes corner
/// positions as full-precision floats, so an exact comparison would call a
/// perfectly ordinary slice warped.
const AXIS_EPS: f32 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("that file is not valid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error(
        "no <ScreenSetup> in that file. A Resolume advanced output is either an \
         AdvancedOutput.xml (root <ScreenSetup>) or a preset (root <XmlState>)"
    )]
    NotAnAdvancedOutput,
    #[error("that advanced output has no screens in it")]
    NoScreens,
}

/// A cheap check for whether a file is worth handing to [`parse`].
pub fn is_resolume_xml(text: &str) -> bool {
    text.contains("<ScreenSetup") || text.contains("<XmlState")
}

/// Read the four `<v x= y=>` children of a rect element, in file order.
///
/// Resolume writes them top-left, top-right, bottom-right, bottom-left, which is
/// [`Quad`]'s own winding, so the order is preserved rather than sorted. Sorting
/// them into a bounding box here is exactly the information loss this reader
/// exists to avoid.
fn quad_from(el: Node) -> Option<Quad> {
    let pts: Vec<Vec2> = el
        .children()
        .filter(|c| c.has_tag_name("v"))
        .filter_map(|v| {
            let x: f32 = v.attribute("x")?.parse().ok()?;
            let y: f32 = v.attribute("y")?.parse().ok()?;
            Some(Vec2::new(x, y))
        })
        .collect();

    match pts.len() {
        4 => Some(Quad::new(pts[0], pts[1], pts[2], pts[3])),
        // Three points still describe a plane, so infer the fourth by
        // parallelogram rather than discarding the slice.
        3 => Some(Quad::new(pts[0], pts[1], pts[2], pts[0] + pts[2] - pts[1])),
        _ => None,
    }
}

/// The value of a `<Param name="...">` anywhere beneath `node`.
fn param<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.descendants()
        .find(|n| n.has_tag_name("Param") && n.attribute("name") == Some(name))
        .and_then(|n| n.attribute("value"))
}

fn child<'a>(node: Node<'a, 'a>, tag: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|c| c.has_tag_name(tag))
}

/// Resolume stores a name both as an attribute and as a `<Param>`; the Param wins
/// because it is what the operator typed.
fn screen_name(screen: Node, index: usize) -> String {
    param(screen, "Name")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            screen
                .attribute("name")
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Screen {}", index + 1))
}

/// A slice's name, ignoring Resolume's `"Layer"` default.
fn slice_name(slice: Node, index: usize) -> String {
    param(slice, "Name")
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "Layer")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Slice {}", index + 1))
}

struct Device {
    label: Option<String>,
    size: Option<Size>,
}

/// Pull the output device out of a screen.
///
/// Arena writes one of several element types depending on where the screen goes:
/// `OutputDeviceVirtual` carries width/height; `OutputDeviceCapture` (a DeckLink
/// or similar) and `OutputDeviceDisplay` (a real monitor) carry a machine-specific
/// `idHash` and no dimensions at all. Anything unrecognised is still reported by
/// tag name, because knowing a device exists is useful even when its size is not
/// readable.
fn output_device(screen: Node) -> Device {
    let Some(container) = child(screen, "OutputDevice") else {
        return Device {
            label: None,
            size: None,
        };
    };
    let Some(el) = container
        .children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("OutputDevice"))
    else {
        return Device {
            label: None,
            size: None,
        };
    };

    let label = el
        .attribute("name")
        .or_else(|| el.attribute("deviceId"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            Some(
                el.tag_name()
                    .name()
                    .trim_start_matches("OutputDevice")
                    .to_owned(),
            )
        });

    let size = match (
        el.attribute("width").and_then(|v| v.parse::<u32>().ok()),
        el.attribute("height").and_then(|v| v.parse::<u32>().ok()),
    ) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(Size::new(w, h)),
        _ => None,
    };

    Device { label, size }
}

fn bounding_box(quads: &[Quad]) -> Option<Rect> {
    quads.iter().map(|q| q.bounds()).reduce(|a, b| a.union(&b))
}

/// Parse an Advanced Output.
///
/// `file_name` is only used to name the resulting project.
pub fn parse(text: &str, file_name: &str) -> Result<SliceMap, ImportError> {
    let doc = Document::parse(text)?;
    let root = doc.root_element();

    let setup = if root.has_tag_name("ScreenSetup") {
        root
    } else {
        root.descendants()
            .find(|n| n.has_tag_name("ScreenSetup"))
            .ok_or(ImportError::NotAnAdvancedOutput)?
    };

    let mut warnings = Vec::new();

    let version_label = doc
        .descendants()
        .find(|n| n.has_tag_name("versionInfo"))
        .map(|v| {
            format!(
                "{} {}.{}",
                v.attribute("name").unwrap_or("Resolume"),
                v.attribute("majorVersion").unwrap_or("?"),
                v.attribute("minorVersion").unwrap_or("?"),
            )
        });

    let composition = child(setup, "CurrentCompositionTextureSize").and_then(|c| {
        match (
            c.attribute("width").and_then(|v| v.parse::<u32>().ok()),
            c.attribute("height").and_then(|v| v.parse::<u32>().ok()),
        ) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(Size::new(w, h)),
            _ => None,
        }
    });

    let screen_nodes: Vec<Node> = child(setup, "screens")
        .map(|s| s.children().filter(|c| c.has_tag_name("Screen")).collect())
        .unwrap_or_default();

    if screen_nodes.is_empty() {
        return Err(ImportError::NoScreens);
    }

    let mut screens = Vec::new();
    let mut warped_slices = 0usize;
    let mut oriented_slices = 0usize;
    let mut warper_slices = 0usize;

    for (si, screen_node) in screen_nodes.iter().enumerate() {
        let mut notes = Vec::new();
        let name = screen_name(*screen_node, si);

        // Any layer carrying an OutputRect counts, so PolySlices and whatever
        // Arena adds later are picked up without this needing their tag names.
        let layers: Vec<Node> = child(*screen_node, "layers")
            .map(|l| {
                l.children()
                    .filter(|c| c.is_element() && child(*c, "OutputRect").is_some())
                    .collect()
            })
            .unwrap_or_default();

        let mut slices = Vec::new();
        let mut out_quads = Vec::new();

        for (li, layer) in layers.iter().enumerate() {
            let Some(output) = child(*layer, "OutputRect").and_then(quad_from) else {
                continue;
            };
            // A slice with no InputRect reads its whole source, which is the
            // composition rect for lack of anything better.
            let input = child(*layer, "InputRect")
                .and_then(quad_from)
                .unwrap_or(output);

            if !output.is_axis_aligned(AXIS_EPS) || !input.is_axis_aligned(AXIS_EPS) {
                warped_slices += 1;
            }

            let orientation = child(*layer, "OutputRect")
                .and_then(|r| r.attribute("orientation"))
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(0);
            if orientation != 0 {
                oriented_slices += 1;
            }

            if child(*layer, "Warper")
                .map(|w| {
                    w.descendants()
                        .any(|n| n.has_tag_name("BezierWarper") || n.has_tag_name("Homography"))
                })
                .unwrap_or(false)
                && !is_identity_warper(*layer, output)
            {
                warper_slices += 1;
            }

            let enabled = param(*layer, "Enabled").map(|v| v != "0").unwrap_or(true);

            out_quads.push(output);
            slices.push(Slice {
                id: layer
                    .attribute("uniqueId")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{si}-{li}")),
                name: slice_name(*layer, li),
                input,
                output,
                enabled,
                orientation,
            });
        }

        let device = output_device(*screen_node);
        let bounds = bounding_box(&out_quads);

        let (raster, raster_source) = match (device.size, bounds) {
            (Some(size), _) => (size, RasterSource::Declared),
            (None, Some(b)) => {
                // Slices are positioned in raster coordinates, so the raster must
                // extend to the far edge of the furthest slice — but its origin is
                // 0,0 even when no slice starts there. Use the extent, not the
                // box's own size.
                notes.push(format!(
                    "Raster inferred from slice positions — this screen's output device ({}) \
                     does not record its size. Check it against the real output.",
                    device.label.as_deref().unwrap_or("unknown")
                ));
                (
                    Size::new(
                        b.right().round().max(1.0) as u32,
                        b.bottom().round().max(1.0) as u32,
                    ),
                    RasterSource::SliceBounds,
                )
            }
            (None, None) => {
                notes.push(
                    "No slices and no device size — fell back to the composition size. \
                     Set this by hand."
                        .into(),
                );
                (
                    composition.unwrap_or(Size::new(1920, 1080)),
                    RasterSource::Fallback,
                )
            }
        };

        screens.push(Screen {
            id: screen_node
                .attribute("uniqueId")
                .map(str::to_owned)
                .unwrap_or_else(|| format!("screen-{si}")),
            name,
            raster,
            raster_source,
            device: device.label,
            slices,
            notes,
        });
    }

    if let Some(v) = version_label {
        warnings.push(format!("Read as {v}."));
    }
    if warped_slices > 0 {
        warnings.push(format!(
            "{warped_slices} slice{} rotated or corner-pinned. Their corners are \
             reproduced, but any soft-edge blend is not.",
            if warped_slices == 1 { " is" } else { "s are" }
        ));
    }
    if oriented_slices > 0 {
        warnings.push(format!(
            "{oriented_slices} slice{} a non-zero orientation (flip/rotate). \
             UnMapper does not yet apply it, so those slices will appear unflipped.",
            if oriented_slices == 1 {
                " has"
            } else {
                "s have"
            }
        ));
    }
    if warper_slices > 0 {
        warnings.push(format!(
            "{warper_slices} slice{} a non-identity warp (bezier mesh or homography). \
             UnMapper renders the four corners only, so the warp will not be reproduced.",
            if warper_slices == 1 { " has" } else { "s have" }
        ));
    }
    let inferred = screens
        .iter()
        .filter(|s| s.raster_source == RasterSource::SliceBounds)
        .count();
    if inferred > 0 {
        warnings.push(format!(
            "{inferred} of {} output rasters were inferred from slice bounds rather than \
             read from the file. Confirm each one before trusting the layout.",
            screens.len()
        ));
    }

    let project_name = file_name
        .rsplit('/')
        .next()
        .unwrap_or(file_name)
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);

    Ok(SliceMap {
        project_name: if project_name.is_empty() {
            "advanced-output".into()
        } else {
            project_name.to_owned()
        },
        composition,
        screens,
        warnings,
    })
}

/// Whether a slice's `<Warper>` is the untouched default.
///
/// Arena writes a full Warper for every slice whether or not the operator has
/// touched it, so the presence of one means nothing. A default warper's homography
/// maps the output rect to itself, and its bezier grid is a regular lattice across
/// that rect. Testing the homography is enough and is far cheaper than testing
/// the grid.
fn is_identity_warper(layer: Node, output: Quad) -> bool {
    let Some(homography) = layer.descendants().find(|n| n.has_tag_name("Homography")) else {
        return true;
    };
    let src = child(homography, "src").and_then(quad_from);
    let dst = child(homography, "dst").and_then(quad_from);
    match (src, dst) {
        (Some(s), Some(d)) => {
            let same = |a: Vec2, b: Vec2| (a - b).length() <= AXIS_EPS;
            let identity =
                same(s.tl, d.tl) && same(s.tr, d.tr) && same(s.br, d.br) && same(s.bl, d.bl);
            // A homography that is the identity, or that simply restates the
            // output rect, is doing nothing.
            identity || (same(d.tl, output.tl) && same(d.br, output.br))
        }
        _ => true,
    }
}
