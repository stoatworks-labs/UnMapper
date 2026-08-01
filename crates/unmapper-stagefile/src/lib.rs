//! The UnMapper **stage file** — a legible XML description of a rig.
//!
//! ```xml
//! <UnMapperStage version="1" name="Main Wall">
//!   <VirtualRaster width="1920" height="1080"/>
//!   <Sources>
//!     <Source id="src-9001" name="LED Processor 1" enabled="true">
//!       <Ndi name="STUDIO (Arena - Screen 1)"/>
//!       <ScreenRaster screen="9001"/>
//!       <Expected width="1920" height="1080"/>
//!     </Source>
//!   </Sources>
//!   <Panels>
//!     <Panel id="wall-left" name="Wall Left" enabled="true">
//!       <Pixels width="960" height="1080"/>
//!       <Layout x="0" y="0" width="960" height="1080"/>
//!       <Placement>
//!         <Translation x="-1.24" y="1.4" z="0"/>
//!         <Rotation x="0" y="0.21" z="0" w="0.97"/>
//!         <Size width="2.49" height="2.8"/>
//!       </Placement>
//!     </Panel>
//!   </Panels>
//!   <Bindings>
//!     <Binding panel="wall-left" source="src-9001" slice="9101">
//!       <SourceQuad>
//!         <v x="0" y="0"/><v x="960" y="0"/><v x="960" y="1080"/><v x="0" y="1080"/>
//!       </SourceQuad>
//!     </Binding>
//!   </Bindings>
//! </UnMapperStage>
//! ```
//!
//! # Why real XML rather than JSON in a CDATA block
//!
//! The sibling `openstage` writes its project files as XML wrapping each section's
//! JSON in CDATA, and that is right *there*: those sections are tagged enums,
//! HashMaps and fixed arrays, and a second hand-written XML mapping of them would
//! drift from the serde one.
//!
//! A stage is different. It is genuinely tree-shaped — panels holding placements
//! holding vectors, bindings holding quads — and the whole point of asking for an
//! XML format is a file an operator can read, diff and hand-edit. A CDATA-wrapped
//! JSON blob would be XML in name only and unreadable in practice. So this is a
//! real mapping, and the tests below are what stop it drifting.
//!
//! # Conventions
//!
//! - A quad is four `<v x= y=>` children in top-left, top-right, bottom-right,
//!   bottom-left order — deliberately the same shape and order Resolume uses for
//!   its own `InputRect` and `OutputRect`.
//! - Whole numbers are written without a decimal point, so it reads as `960`
//!   rather than `960.0`.
//! - Sections with nothing in them are omitted rather than written empty.
//! - Reading is forgiving (unknown elements ignored, defaults applied); a wrong
//!   *shape* is not, because that renders wrongly rather than failing.

mod read;
mod write;

use unmapper_core::Show;

/// Bumped when the on-disk shape changes incompatibly.
pub const STAGE_FORMAT: u32 = 1;

/// The conventional extension. Not enforced.
pub const STAGE_EXTENSION: &str = "unmapper.xml";

pub(crate) const RASTER_DECLARED: &str = "declared";
pub(crate) const RASTER_SLICE_BOUNDS: &str = "slice-bounds";
pub(crate) const RASTER_FALLBACK: &str = "fallback";

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("that file is not valid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("could not write the stage: {0}")]
    Write(#[from] std::io::Error),
    #[error(
        "that is not an UnMapper stage — the root element is <{found}>, expected <UnMapperStage>"
    )]
    NotAStage { found: String },
    #[error("that stage was written by a newer UnMapper (version {found}, this build reads {supported})")]
    TooNew { found: u32, supported: u32 },
    #[error("this stage is malformed: {0}")]
    Malformed(String),
}

/// Serialise a show as stage XML.
pub fn to_xml(show: &Show) -> Result<String, StageError> {
    Ok(write::to_xml(show)?)
}

/// Parse stage XML.
pub fn from_xml(text: &str) -> Result<Show, StageError> {
    read::from_xml(text)
}

/// A cheap check for whether a file is a stage rather than, say, a Resolume
/// Advanced Output — useful when one "Open…" accepts both.
pub fn is_stage_xml(text: &str) -> bool {
    text.contains("<UnMapperStage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use unmapper_core::{
        Backdrop, Binding, Camera, Model3d, Output, OutputTarget, OutputView, Panel, Quad,
        RasterSource, Rect, Screen, Size, Slice, SliceMap, Source, SourceKind, SourceSpace, Vec2,
        Vec3,
    };

    /// A show exercising every branch of the format at once.
    fn full_show() -> Show {
        let mut show = Show {
            name: "Main Wall".into(),
            virtual_raster: Size::new(1920, 1080),
            ..Default::default()
        };

        show.geometry.backdrop = Some(Backdrop {
            path: "art/stage mock-up.png".into(),
            rect: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            opacity: 0.45,
        });
        show.geometry.model = Some(Model3d {
            path: "cad/stage.glb".into(),
            scale: 0.001,
            rotation: glam::Quat::from_rotation_y(0.5),
            translation: Vec3::new(0.0, -1.0, 2.5),
        });

        show.sources.push(Source {
            id: "src-9001".into(),
            name: "LED Processor 1".into(),
            kind: SourceKind::Ndi {
                name: "STUDIO (Arena - Screen 1)".into(),
            },
            space: SourceSpace::ScreenRaster {
                screen_id: "9001".into(),
            },
            expected: Some(Size::new(1920, 1080)),
            enabled: true,
        });
        show.sources.push(Source {
            id: "src-still".into(),
            name: "Holding slide".into(),
            kind: SourceKind::Still {
                path: "media/holding.png".into(),
            },
            space: SourceSpace::Composition,
            expected: None,
            enabled: false,
        });

        for (i, id) in ["wall-left", "wall-right"].iter().enumerate() {
            let mut panel = Panel::from_layout(
                *id,
                format!("Wall {}", if i == 0 { "Left" } else { "Right" }),
                Size::new(960, 1080),
                Rect::new(i as f32 * 960.0, 0.0, 960.0, 1080.0),
                2.6,
            );
            panel.placement.rotation = glam::Quat::from_rotation_y(if i == 0 { 0.4 } else { -0.4 });
            panel.enabled = i == 0;
            show.panels.push(panel);
            show.bindings.push(Binding {
                panel_id: (*id).into(),
                source_id: "src-9001".into(),
                source_quad: Quad::from_rect(Rect::new(i as f32 * 960.0, 0.0, 960.0, 1080.0)),
                slice_id: Some(format!("910{i}")),
            });
        }

        show.outputs.push(Output {
            id: "out-1".into(),
            name: "Stage left monitor".into(),
            target: OutputTarget::Display {
                index: 1,
                fullscreen: true,
            },
            view: OutputView::Emulation {
                region: Rect::new(0.0, 0.0, 960.0, 1080.0),
            },
            size: Size::new(960, 1080),
            enabled: true,
        });
        show.outputs.push(Output {
            id: "out-2".into(),
            name: "Previz to NDI".into(),
            target: OutputTarget::Ndi {
                name: "UnMapper Previz".into(),
            },
            view: OutputView::Previz {
                camera: Camera {
                    position: Vec3::new(0.0, 1.7, 9.0),
                    ..Default::default()
                },
            },
            size: Size::new(1280, 720),
            enabled: false,
        });

        show.slice_map = Some(SliceMap {
            project_name: "two-panel-wall".into(),
            composition: Some(Size::new(1920, 1080)),
            screens: vec![Screen {
                id: "9001".into(),
                name: "LED Processor 1".into(),
                raster: Size::new(1920, 1080),
                raster_source: RasterSource::Declared,
                device: Some("VirtualLED1".into()),
                slices: vec![Slice {
                    id: "9101".into(),
                    name: "Wall Left".into(),
                    input: Quad::from_rect(Rect::new(0.0, 0.0, 960.0, 1080.0)),
                    // A corner-pinned slice, so the four-corner path is exercised.
                    output: Quad::new(
                        Vec2::new(10.0, 0.0),
                        Vec2::new(950.0, 4.5),
                        Vec2::new(960.0, 1080.0),
                        Vec2::new(0.0, 1075.5),
                    ),
                    enabled: true,
                    orientation: 2,
                }],
                notes: vec!["Raster inferred from slice positions.".into()],
            }],
            warnings: vec!["Read as Resolume Arena 7.27.".into()],
        });

        show
    }

    /// Compare two shows field by field, since a whole-struct assert on failure
    /// prints two screenfuls of Debug and says nothing about what differed.
    fn assert_same(a: &Show, b: &Show) {
        assert_eq!(a.name, b.name, "name");
        assert_eq!(a.virtual_raster, b.virtual_raster, "virtual raster");
        assert_eq!(a.sources, b.sources, "sources");
        assert_eq!(a.panels.len(), b.panels.len(), "panel count");
        for (x, y) in a.panels.iter().zip(&b.panels) {
            assert_eq!(x.id, y.id, "panel id");
            assert_eq!(x.name, y.name, "panel {} name", x.id);
            assert_eq!(x.pixels, y.pixels, "panel {} pixels", x.id);
            assert_eq!(x.layout, y.layout, "panel {} layout", x.id);
            assert_eq!(x.enabled, y.enabled, "panel {} enabled", x.id);
            assert_eq!(
                x.placement.translation, y.placement.translation,
                "panel {} translation",
                x.id
            );
            assert_eq!(x.placement.size, y.placement.size, "panel {} size", x.id);
            assert!(
                x.placement.rotation.abs_diff_eq(y.placement.rotation, 1e-7),
                "panel {} rotation: {:?} vs {:?}",
                x.id,
                x.placement.rotation,
                y.placement.rotation
            );
        }
        assert_eq!(a.bindings, b.bindings, "bindings");
        assert_eq!(a.outputs, b.outputs, "outputs");
        assert_eq!(a.geometry.backdrop, b.geometry.backdrop, "backdrop");
        assert_eq!(a.slice_map, b.slice_map, "slice map");
    }

    #[test]
    fn a_full_stage_survives_the_round_trip() {
        let show = full_show();
        let xml = to_xml(&show).unwrap();
        let back = from_xml(&xml).unwrap();
        assert_same(&show, &back);
    }

    #[test]
    fn writing_twice_gives_byte_identical_output() {
        // Stage files go in git next to a show. Non-deterministic output would
        // make every save a spurious diff.
        let show = full_show();
        let once = to_xml(&show).unwrap();
        let twice = to_xml(&from_xml(&once).unwrap()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn the_file_is_actually_legible() {
        let xml = to_xml(&full_show()).unwrap();
        // Indented, not one long line.
        assert!(xml.lines().count() > 40, "should be pretty-printed");
        // Whole numbers have no pointless decimal.
        assert!(xml.contains(r#"<Layout x="0" y="0" width="960" height="1080"/>"#));
        // Quads use Resolume's own <v> convention.
        assert!(xml.contains(r#"<v x="960" y="1080"/>"#));
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    }

    #[test]
    fn corner_pinned_slices_keep_all_four_corners() {
        let show = full_show();
        let back = from_xml(&to_xml(&show).unwrap()).unwrap();
        let original = &show.slice_map.as_ref().unwrap().screens[0].slices[0];
        let reread = &back.slice_map.as_ref().unwrap().screens[0].slices[0];
        assert_eq!(original.output, reread.output);
        assert!(!reread.output.is_axis_aligned(0.5), "the warp must survive");
        assert_eq!(reread.orientation, 2, "orientation must survive");
    }

    #[test]
    fn special_characters_in_names_are_escaped() {
        // NDI names carry parentheses routinely, and a stage might be called
        // something with an ampersand in it.
        let mut show = full_show();
        show.name = r#"Ben & Jerry's <Main> "Wall""#.into();
        show.sources[0].kind = SourceKind::Ndi {
            name: "PC-1 (Arena - A&B)".into(),
        };
        let xml = to_xml(&show).unwrap();
        let back = from_xml(&xml).unwrap();
        assert_eq!(back.name, show.name);
        assert_eq!(back.sources[0].kind, show.sources[0].kind);
    }

    #[test]
    fn a_minimal_hand_written_stage_loads() {
        // The point of an XML format: this is a reasonable thing to type by hand,
        // and every omitted attribute should take a sensible default.
        let xml = r#"<?xml version="1.0"?>
            <UnMapperStage name="Scratch">
              <VirtualRaster width="1920" height="1080"/>
              <Sources><Source id="s" name="Feed"><Ndi name="PC (Arena)"/></Source></Sources>
              <Panels>
                <Panel id="p" name="Panel">
                  <Pixels width="1920" height="1080"/>
                  <Layout x="0" y="0" width="1920" height="1080"/>
                </Panel>
              </Panels>
              <Bindings>
                <Binding panel="p" source="s">
                  <SourceQuad>
                    <v x="0" y="0"/><v x="1920" y="0"/><v x="1920" y="1080"/><v x="0" y="1080"/>
                  </SourceQuad>
                </Binding>
              </Bindings>
            </UnMapperStage>"#;
        let show = from_xml(xml).unwrap();
        assert_eq!(show.name, "Scratch");
        assert_eq!(show.panels.len(), 1);
        assert!(show.panels[0].enabled, "enabled should default to true");
        assert!(show.sources[0].enabled);
        assert_eq!(show.sources[0].space, SourceSpace::Composition);
        assert!(show
            .validate()
            .iter()
            .all(|p| p.severity != unmapper_core::Severity::Error));
    }

    #[test]
    fn unknown_elements_are_ignored_rather_than_refused() {
        // Forward compatibility: a stage from a newer build at the same version
        // should still load what it can.
        let xml = to_xml(&full_show())
            .unwrap()
            .replace("<Panels>", "<SomethingNew foo=\"1\"/><Panels>");
        assert!(from_xml(&xml).is_ok());
    }

    #[test]
    fn a_binding_naming_a_missing_panel_is_refused() {
        // This would otherwise render as a silently absent panel.
        let xml = to_xml(&full_show())
            .unwrap()
            .replace(r#"panel="wall-left""#, r#"panel="does-not-exist""#);
        let err = from_xml(&xml).unwrap_err();
        assert!(
            matches!(&err, StageError::Malformed(m) if m.contains("does-not-exist")),
            "got {err}"
        );
    }

    #[test]
    fn a_quad_with_the_wrong_number_of_corners_is_refused() {
        let xml = to_xml(&full_show())
            .unwrap()
            .replacen(r#"<v x="0" y="0"/>"#, "", 1);
        assert!(matches!(from_xml(&xml), Err(StageError::Malformed(_))));
    }

    #[test]
    fn the_wrong_root_element_says_so_plainly() {
        let err =
            from_xml(r#"<?xml version="1.0"?><XmlState><ScreenSetup/></XmlState>"#).unwrap_err();
        assert!(
            matches!(&err, StageError::NotAStage { found } if found == "XmlState"),
            "got {err}"
        );
        assert!(!is_stage_xml("<XmlState/>"));
    }

    #[test]
    fn a_stage_from_the_future_is_refused_rather_than_half_read() {
        let xml = to_xml(&full_show())
            .unwrap()
            .replace(r#"version="1""#, r#"version="99""#);
        assert!(matches!(from_xml(&xml), Err(StageError::TooNew { .. })));
    }

    #[test]
    fn a_hand_edited_rotation_is_normalised_rather_than_scaling_the_panel() {
        // A non-unit quaternion silently scales whatever it rotates, so a
        // plausible hand-typed value must not quietly resize a wall.
        let xml = to_xml(&full_show()).unwrap().replace(
            r#"<Rotation x="0" y="0.19866933" z="0" w="0.9800666"/>"#,
            r#"<Rotation x="0" y="2" z="0" w="2"/>"#,
        );
        let show = from_xml(&xml).unwrap();
        for p in &show.panels {
            assert!(
                (p.placement.rotation.length() - 1.0).abs() < 1e-5,
                "rotation must be unit length, got {}",
                p.placement.rotation.length()
            );
        }
    }

    #[test]
    fn an_output_with_no_target_is_refused() {
        let xml = to_xml(&full_show())
            .unwrap()
            .replace(r#"<Display index="1" fullscreen="true"/>"#, "");
        assert!(matches!(from_xml(&xml), Err(StageError::Malformed(_))));
    }

    #[test]
    fn an_unrecognised_raster_source_does_not_become_trustworthy() {
        // "declared" means the file stated the size and it can be trusted.
        // Anything unreadable must fall to the weakest option, never the strongest.
        let xml = to_xml(&full_show())
            .unwrap()
            .replace(r#"rasterSource="declared""#, r#"rasterSource="who-knows""#);
        let show = from_xml(&xml).unwrap();
        let screen = &show.slice_map.as_ref().unwrap().screens[0];
        assert_eq!(screen.raster_source, RasterSource::Fallback);
        assert!(screen.raster_source.needs_confirmation());
    }
}
