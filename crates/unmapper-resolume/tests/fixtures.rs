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
//!
//! The last two matter because UnMapper must read what the rest of the fleet
//! writes; a change that breaks them breaks the fleet's own round trip.

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
    let map = parse(&fixture("resolume-arena-preset.xml"), "preset").unwrap();
    assert!(
        !map.warnings.iter().any(|w| w.contains("non-identity warp")),
        "untouched warpers must not warn, got {:?}",
        map.warnings
    );
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
