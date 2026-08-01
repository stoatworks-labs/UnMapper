//! Real GPU rendering, verified by reading the pixels back.
//!
//! These run headless — no window, no surface — so they work in CI and over SSH.
//! They are the only check that the shader, the vertex transform, the UV mapping
//! and the y-flip all agree. Geometry unit tests cannot catch a flipped canvas;
//! only looking at the output can.
//!
//! # How the fixtures avoid filtering ambiguity
//!
//! The sampler filters linearly, so a test that samples across a texel boundary
//! gets a blend and an unstable assertion — a 2x2 source is all boundary, and
//! reads a few percent off every pure colour. The fixtures here use a source with
//! large flat quadrants instead, so the sample point sits deep inside one of them
//! where the filter has nothing to blend with.

use unmapper_core::{Binding, Panel, Quad, Rect, Show, Size, SourceKind, SourceSpace};
use unmapper_render::{
    build_canvas_scene, build_previz_scene, FrameUpload, Gpu, RenderTarget, Renderer,
    SourceTextures,
};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// The side of the test source, in pixels. Each quadrant is `SRC/2` square.
///
/// Deliberately not 2x2. With a 2x2 source every sample sits near a texel
/// boundary, so linear filtering blends the neighbours in and the assertions come
/// out a few percent off a pure colour. Large quadrants put the sample point deep
/// inside one of them, where the filter has nothing to blend with.
const SRC: u32 = 64;

/// A `SRC`-square source in four quadrants: TL red, TR green, BL blue, BR white.
fn quad_source() -> Vec<u8> {
    let half = SRC / 2;
    let mut data = Vec::with_capacity((SRC * SRC * 4) as usize);
    for y in 0..SRC {
        for x in 0..SRC {
            let c = match (x < half, y < half) {
                (true, true) => RED,
                (false, true) => GREEN,
                (true, false) => BLUE,
                (false, false) => WHITE,
            };
            data.extend_from_slice(&c);
        }
    }
    data
}

/// A source of one flat colour.
fn flat_source(c: [u8; 4]) -> Vec<u8> {
    c.repeat((SRC * SRC) as usize)
}

/// The source rect covering one quadrant.
fn quadrant(right: bool, bottom: bool) -> Rect {
    let h = (SRC / 2) as f32;
    Rect::new(
        if right { h } else { 0.0 },
        if bottom { h } else { 0.0 },
        h,
        h,
    )
}

fn gpu() -> Gpu {
    Gpu::new_blocking().expect("this machine needs a working GPU adapter for these tests")
}

fn source(show: &mut Show, id: &str, size: Size) {
    show.sources.push(unmapper_core::Source {
        id: id.into(),
        name: id.into(),
        kind: SourceKind::TestPattern,
        space: SourceSpace::Composition,
        expected: Some(size),
        enabled: true,
    });
}

/// Add a panel at `layout` sampling `src` (in source pixels) from `source_id`.
fn panel(show: &mut Show, id: &str, layout: Rect, source_id: &str, src: Rect) {
    show.panels.push(Panel::from_layout(
        id,
        id,
        Size::new(layout.width as u32, layout.height as u32),
        layout,
        2.6,
    ));
    show.bindings.push(Binding {
        panel_id: id.into(),
        source_id: source_id.into(),
        source_quad: Quad::from_rect(src),
        slice_id: None,
    });
}

/// The pixel at (x, y) of a readback.
fn px(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

fn assert_close(got: [u8; 4], want: [u8; 4], what: &str) {
    let ok = got
        .iter()
        .zip(want.iter())
        .all(|(a, b)| (*a as i16 - *b as i16).abs() <= 2);
    assert!(ok, "{what}: expected {want:?}, got {got:?}");
}

#[test]
fn a_panel_samples_the_quadrant_its_binding_names() {
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &quad_source(),
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(20, 10),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    // Left panel takes the source's top-left texel, right panel its top-right.
    panel(
        &mut show,
        "left",
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "s",
        quadrant(false, false),
    );
    panel(
        &mut show,
        "right",
        Rect::new(10.0, 0.0, 10.0, 10.0),
        "s",
        quadrant(true, false),
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    assert_close(
        px(&data, 20, 5, 5),
        RED,
        "left panel should show the TL texel",
    );
    assert_close(
        px(&data, 20, 15, 5),
        GREEN,
        "right panel should show the TR texel",
    );
}

#[test]
fn the_canvas_is_not_flipped_vertically() {
    // The single easiest thing to get wrong: canvas space has y down, NDC has y
    // up. A panel at the top of the layout must appear at the top of the image.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &quad_source(),
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(10, 20),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    // Top of the canvas takes the source's TOP-left texel (red);
    // bottom takes the BOTTOM-left (blue).
    panel(
        &mut show,
        "top",
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "s",
        quadrant(false, false),
    );
    panel(
        &mut show,
        "bottom",
        Rect::new(0.0, 10.0, 10.0, 10.0),
        "s",
        quadrant(false, true),
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    // Row 0 is the top of the image.
    assert_close(
        px(&data, 10, 5, 5),
        RED,
        "canvas row 0 must be the top panel",
    );
    assert_close(
        px(&data, 10, 5, 15),
        BLUE,
        "canvas last row must be the bottom panel",
    );
}

#[test]
fn a_bgra_source_renders_the_same_colours_as_an_rgba_one() {
    // The wire format is handled by choosing the texture format, not by
    // rewriting pixels — so a swizzle bug here would show as swapped channels.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    // The same image as quad_source(), byte-swapped into BGRA.
    let mut bgra = quad_source();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: true,
            data: &bgra,
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(10, 10),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "s",
        quadrant(false, false),
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    assert_close(
        px(&data, 10, 5, 5),
        RED,
        "BGRA red must render as red, not blue",
    );
}

#[test]
fn an_unbound_panel_renders_dim_rather_than_invisible() {
    let gpu = gpu();
    let textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    let mut show = Show {
        virtual_raster: Size::new(10, 10),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    // No upload, so there is no texture for "s".
    panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "s",
        quadrant(false, false),
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    let got = px(&data, 10, 5, 5);
    assert!(
        got[0] > 10 && got[0] < 60,
        "an unbound panel should be visibly dim, not black or bright — got {got:?}"
    );
}

#[test]
fn empty_canvas_is_black_rather_than_uninitialised() {
    let gpu = gpu();
    let textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    let show = Show {
        virtual_raster: Size::new(8, 8),
        ..Default::default()
    };
    let scene = build_canvas_scene(&show, &textures);
    assert!(scene.is_empty());

    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    assert_eq!(px(&data, 8, 4, 4), [0, 0, 0, 255]);
}

#[test]
fn previz_draws_a_panel_the_camera_is_pointed_at() {
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &quad_source(),
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(100, 100),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 100.0, 100.0),
        "s",
        quadrant(false, false),
    );
    show.arrange_panels_from_layout();

    // Point the camera straight at the panel's centre from in front of it.
    let centre = show.panel("p").unwrap().placement.translation;
    let camera = unmapper_core::Camera {
        position: centre + glam::Vec3::new(0.0, 0.0, 3.0),
        target: centre,
        ..Default::default()
    };

    let size = Size::new(64, 64);
    let scene = build_previz_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, size, "previz");
    renderer.render_previz(&gpu, &target.view, size, &camera, &scene, &textures);

    let data = target.read_rgba(&gpu);
    assert_close(
        px(&data, 64, 32, 32),
        RED,
        "the panel should fill the centre of the previz frame",
    );
}

#[test]
fn a_corner_pinned_source_quad_still_samples_inside_the_source() {
    // The projective path. A trapezoid source quad must not produce NaNs or
    // sample outside the texture — either would show as black or garbage.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    // A uniform source, so any correctly-sampled pixel is exactly this colour
    // and any sampling fault shows up as something else.
    let uniform = flat_source(WHITE);
    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &uniform,
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(32, 32),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    show.panels.push(Panel::from_layout(
        "p",
        "p",
        Size::new(32, 32),
        Rect::new(0.0, 0.0, 32.0, 32.0),
        2.6,
    ));
    show.bindings.push(Binding {
        panel_id: "p".into(),
        source_id: "s".into(),
        // A keystoned slice: the top edge is half the width of the bottom.
        source_quad: Quad::new(
            glam::Vec2::new(SRC as f32 * 0.25, 0.0),
            glam::Vec2::new(SRC as f32 * 0.75, 0.0),
            glam::Vec2::new(SRC as f32, SRC as f32),
            glam::Vec2::new(0.0, SRC as f32),
        ),
        slice_id: None,
    });

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    // Sample across the whole panel, including the diagonal where a bilinear
    // map would disagree with a projective one.
    for (x, y) in [(4, 4), (16, 4), (28, 4), (16, 16), (4, 28), (28, 28)] {
        assert_close(
            px(&data, 32, x, y),
            WHITE,
            &format!("warped sample at {x},{y}"),
        );
    }
}

#[test]
fn two_sources_become_two_draw_groups() {
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    let red = flat_source(RED);
    let green = flat_source(GREEN);
    textures.upload(
        &gpu,
        "a",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &red,
            sequence: 1,
        },
    );
    textures.upload(
        &gpu,
        "b",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &green,
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(20, 10),
        ..Default::default()
    };
    source(&mut show, "a", Size::new(SRC, SRC));
    source(&mut show, "b", Size::new(SRC, SRC));
    panel(
        &mut show,
        "pa",
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "a",
        Rect::new(0.0, 0.0, SRC as f32, SRC as f32),
    );
    panel(
        &mut show,
        "pb",
        Rect::new(10.0, 0.0, 10.0, 10.0),
        "b",
        Rect::new(0.0, 0.0, SRC as f32, SRC as f32),
    );

    let scene = build_canvas_scene(&show, &textures);
    assert_eq!(scene.groups.len(), 2, "one group per source");

    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    assert_close(px(&data, 20, 5, 5), RED, "left panel takes source a");
    assert_close(px(&data, 20, 15, 5), GREEN, "right panel takes source b");
}
