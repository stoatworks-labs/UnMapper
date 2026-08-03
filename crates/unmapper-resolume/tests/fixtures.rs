//! The importer against real files.
//!
//! Four fixtures, chosen to cover both root elements and both raster paths:
//!
//! | Fixture | Root | Exercises |
//! |---|---|---|
//! | `resolume-arena-preset.xml` | `XmlState` | Arena 7.27 preset, declared raster |
//! | `resolume-arena-preferences.xml` | `ScreenSetup` | a capture device with **no** size — the inferred-raster path |
//! | `blend-calc-export.xml` | `XmlState` | six screens, written by the sibling `blend-calc` |
//! | `pixel-peeker-export.xml` | `XmlState` | eleven slices on one screen, written by `pixel-peeker` |
//! | `warped-lattice-synthetic.xml` | `XmlState` | a bowed warp lattice — **hand-authored, not from Arena** |
//!
//! The middle two matter because UnMapper must read what the rest of the fleet
//! writes; a change that breaks them breaks the fleet's own round trip.
//!
//! The last one is the odd one out and its header says so: every real Advanced
//! Output available on this machine has an untouched lattice on every slice, so
//! there was no warped file to test against and this one was made by bowing a
//! copy of the real preset by hand. It pins UnMapper's *reading* of a lattice,
//! and cannot pin what Arena actually writes when an operator warps a slice.
//! Replace it the moment a genuinely warped export exists.

use unmapper_core::{RasterSource, Size};
use unmapper_resolume::{is_resolume_xml, parse, ImportError};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn arena_preset_reads_one_screen_of_three_slices() {
    let map = parse(
        &fixture("resolume-arena-preset.xml"),
        "resolume-arena-preset.xml",
    )
    .unwrap();

    assert_eq!(map.composition, Some(Size::new(2944, 1280)));
    assert_eq!(map.screens.len(), 1);

    let screen = &map.screens[0];
    assert_eq!(screen.name, "OUTPUT MAP 1");
    // The OutputDeviceVirtual declares 3840x1720, which is bigger than the
    // composition — a real and legitimate case, and the reason the device is
    // trusted over the slice bounds.
    assert_eq!(screen.raster, Size::new(3840, 1720));
    assert_eq!(screen.raster_source, RasterSource::Declared);
    assert_eq!(screen.slices.len(), 3);
    assert!(map
        .warnings
        .iter()
        .any(|w| w.contains("Resolume Arena 7.27")));
}

#[test]
fn slice_corners_survive_in_file_order() {
    let map = parse(&fixture("resolume-arena-preset.xml"), "preset").unwrap();
    let slice = &map.screens[0].slices[0];

    // Read straight off the fixture: 0,0 → 1024,0 → 1024,640 → 0,640.
    assert_eq!(slice.output.tl, glam::Vec2::new(0.0, 0.0));
    assert_eq!(slice.output.tr, glam::Vec2::new(1024.0, 0.0));
    assert_eq!(slice.output.br, glam::Vec2::new(1024.0, 640.0));
    assert_eq!(slice.output.bl, glam::Vec2::new(0.0, 640.0));
    assert!(slice.output.is_axis_aligned(0.5));
    assert!(slice.enabled);
}

#[test]
fn a_capture_device_with_no_size_infers_its_raster_and_says_so() {
    let map = parse(
        &fixture("resolume-arena-preferences.xml"),
        "resolume-arena-preferences.xml",
    )
    .unwrap();

    let screen = &map.screens[0];
    assert_eq!(screen.name, "Trailer Screen");
    // OutputDeviceCapture carries an idHash and no dimensions, so there is
    // nothing to read and the raster has to come from the slices.
    assert_eq!(screen.raster_source, RasterSource::SliceBounds);
    assert!(
        screen.notes.iter().any(|n| n.contains("inferred")),
        "the screen must carry a note explaining the guess, got {:?}",
        screen.notes
    );
    assert!(map
        .warnings
        .iter()
        .any(|w| w.contains("inferred from slice bounds")));
}

#[test]
fn root_screen_setup_and_root_xml_state_both_parse() {
    // Preferences files have ScreenSetup at the root; presets nest it in XmlState.
    let prefs = fixture("resolume-arena-preferences.xml");
    let preset = fixture("resolume-arena-preset.xml");
    assert!(prefs.trim_start().starts_with("<?xml") || prefs.contains("<ScreenSetup"));
    assert!(parse(&prefs, "a").is_ok());
    assert!(parse(&preset, "b").is_ok());
    assert!(is_resolume_xml(&prefs));
    assert!(is_resolume_xml(&preset));
}

#[test]
fn blend_calc_export_reads_back_all_six_projector_screens() {
    let map = parse(&fixture("blend-calc-export.xml"), "blend-calc-export.xml").unwrap();

    assert_eq!(map.composition, Some(Size::new(4992, 1664)));
    assert_eq!(map.screens.len(), 6);
    assert_eq!(map.slice_count(), 6);
    for screen in &map.screens {
        assert_eq!(screen.raster, Size::new(1920, 1200));
        assert_eq!(screen.raster_source, RasterSource::Declared);
        assert_eq!(screen.slices.len(), 1);
    }
    // A 2x3 grid of 1920x1200 projectors, each slice covering its own output.
    assert_eq!(map.approximate_pixel_count(), 6 * 1920 * 1200);
}

#[test]
fn pixel_peeker_export_reads_back_all_eleven_slices() {
    let map = parse(
        &fixture("pixel-peeker-export.xml"),
        "pixel-peeker-export.xml",
    )
    .unwrap();

    assert_eq!(map.composition, Some(Size::new(3200, 1800)));
    assert_eq!(map.screens.len(), 1);
    assert_eq!(map.screens[0].raster, Size::new(3200, 1800));
    assert_eq!(map.screens[0].slices.len(), 11);
    // Every slice should have a distinct id, or bindings would collide on re-import.
    let mut ids: Vec<&str> = map.screens[0]
        .slices
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "slice ids must be unique");
}

#[test]
fn project_name_comes_from_the_file_name_without_its_extension() {
    let map = parse(&fixture("blend-calc-export.xml"), "/tmp/My Rig.xml").unwrap();
    assert_eq!(map.project_name, "My Rig");
}

#[test]
fn a_default_warper_does_not_raise_a_warning() {
    // Arena writes a full <Warper> for every slice whether or not it was touched,
    // so warning on its mere presence would cry wolf on every single file.
    //
    // Every fixture written by a real Arena or by the fleet's own exporters has
    // untouched lattices, so *none* of them may warn or carry a mesh.
    for name in [
        "resolume-arena-preset.xml",
        "resolume-arena-preferences.xml",
        "blend-calc-export.xml",
        "pixel-peeker-export.xml",
    ] {
        let map = parse(&fixture(name), name).unwrap();
        assert!(
            !map.warnings.iter().any(|w| w.contains("lattice")),
            "{name}: untouched warpers must not warn, got {:?}",
            map.warnings
        );
        assert!(
            map.slices().all(|(_, s)| s.warp.is_none()),
            "{name}: an untouched lattice must not become a mesh"
        );
    }
}

#[test]
fn a_warped_lattice_is_read_and_only_on_the_slice_that_has_one() {
    let map = parse(
        &fixture("warped-lattice-synthetic.xml"),
        "warped-lattice-synthetic.xml",
    )
    .unwrap();
    let slices = &map.screens[0].slices;
    assert_eq!(slices.len(), 3);

    let mesh = slices[0]
        .warp
        .as_ref()
        .expect("the bowed slice carries a mesh");
    assert_eq!((mesh.columns, mesh.rows), (4, 4));
    assert_eq!(mesh.points.len(), 16);
    // The fixture bows every column by -48*sin(pi*c/3), so the deepest control
    // point sits ~41.6 px off the regular grid.
    let deviation = mesh.max_deviation(slices[0].output);
    assert!(
        (deviation - 41.57).abs() < 0.5,
        "expected a ~41.6 px bow, got {deviation}"
    );

    // The two ends stay pinned — sin() is zero there — which is what makes this
    // an arc rather than a translation.
    let pinned = |p: Option<glam::Vec2>, at: glam::Vec2| {
        let p = p.expect("a corner control point");
        assert!((p - at).length() < 1e-6, "expected {at:?}, got {p:?}");
    };
    pinned(mesh.point(0, 0), glam::Vec2::new(0.0, 0.0));
    pinned(mesh.point(3, 0), glam::Vec2::new(1024.0, 0.0));

    // Its neighbours are untouched and must not be dragged onto the mesh path.
    assert!(slices[1].warp.is_none());
    assert!(slices[2].warp.is_none());

    assert!(
        map.warnings
            .iter()
            .any(|w| w.contains("warped control lattice") && w.contains("42 px")),
        "the warning should say how far it bows, got {:?}",
        map.warnings
    );
}

#[test]
fn a_dragged_interior_point_is_caught_though_no_corner_moved() {
    // The case the old homography-only check missed entirely: the four corners
    // and the homography are all exactly as Arena wrote them, and only an
    // interior control point has moved.
    let mut text = fixture("resolume-arena-preset.xml");
    let from = r#"<v x="341.3333333333333" y="213.33333333333334"/>"#;
    assert!(text.contains(from), "fixture layout changed");
    text = text.replacen(from, r#"<v x="341.3333333333333" y="293.33333333333334"/>"#, 1);

    let map = parse(&text, "dragged").unwrap();
    let slice = &map.screens[0].slices[0];
    assert!(slice.output.is_axis_aligned(0.5), "no corner moved");
    let mesh = slice.warp.as_ref().expect("a dragged interior point is a warp");
    assert!((mesh.max_deviation(slice.output) - 80.0).abs() < 0.5);
}

#[test]
fn garbage_is_refused_with_a_useful_message() {
    assert!(matches!(
        parse("not xml at all <<<", "x"),
        Err(ImportError::Xml(_))
    ));

    let wrong_shape = r#"<?xml version="1.0"?><SomethingElse><foo/></SomethingElse>"#;
    assert!(matches!(
        parse(wrong_shape, "x"),
        Err(ImportError::NotAnAdvancedOutput)
    ));

    let empty = r#"<?xml version="1.0"?><ScreenSetup><screens/></ScreenSetup>"#;
    assert!(matches!(parse(empty, "x"), Err(ImportError::NoScreens)));

    assert!(!is_resolume_xml("{\"json\": true}"));
}

#[test]
fn every_fixture_produces_a_show_that_validates() {
    use unmapper_core::{Severity, Show};

    for name in [
        "resolume-arena-preset.xml",
        "resolume-arena-preferences.xml",
        "blend-calc-export.xml",
        "pixel-peeker-export.xml",
    ] {
        let map = parse(&fixture(name), name).unwrap();
        let show = Show::from_slice_map(map, unmapper_core::DEFAULT_PITCH_MM);
        let problems = show.validate();
        let errors: Vec<_> = problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{name} produced errors: {errors:?}");
        assert!(!show.panels.is_empty(), "{name} produced no panels");

        // And the show it produces must survive a save/load cycle.
        let json = show.to_json().unwrap();
        assert_eq!(Show::from_json(&json).unwrap(), show, "{name} round trip");
    }
}
