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
        source_mesh: None,
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
    for px in bgra.as_chunks_mut::<4>().0 {
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
    renderer.render_previz(
        &gpu,
        &target.view,
        size,
        unmapper_render::PrevizView::camera_only(&camera),
        &scene,
        &textures,
    );

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
        source_mesh: None,
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

#[test]
fn a_blit_crops_the_region_it_is_given() {
    // The emulation output path: one monitor stands in for a piece of the wall,
    // so the crop must land on exactly the right pixels.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);
    let blit = unmapper_render::Blit::new(&gpu, unmapper_render::TARGET_FORMAT);

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

    // A 40x20 canvas: left half red (source TL), right half green (source TR).
    let mut show = Show {
        virtual_raster: Size::new(40, 20),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    panel(
        &mut show,
        "l",
        Rect::new(0.0, 0.0, 20.0, 20.0),
        "s",
        quadrant(false, false),
    );
    panel(
        &mut show,
        "r",
        Rect::new(20.0, 0.0, 20.0, 20.0),
        "s",
        quadrant(true, false),
    );

    let scene = build_canvas_scene(&show, &textures);
    let canvas = RenderTarget::new(&gpu, show.virtual_raster, "canvas");
    renderer.render_canvas(&gpu, &canvas.view, show.virtual_raster, &scene, &textures);

    let source_bind = blit.source(&gpu, &canvas.view);

    // Output A takes the left half of the canvas, output B the right half.
    for (region, want, what) in [
        (Rect::new(0.0, 0.0, 20.0, 20.0), RED, "left output"),
        (Rect::new(20.0, 0.0, 20.0, 20.0), GREEN, "right output"),
    ] {
        let out = RenderTarget::new(&gpu, Size::new(20, 20), "output");
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        blit.draw(
            &gpu,
            &mut encoder,
            &out.view,
            &source_bind,
            show.virtual_raster,
            region,
        );
        gpu.queue.submit([encoder.finish()]);

        let data = out.read_rgba(&gpu);
        // Every pixel of the output, not just the middle: a crop that is off by
        // even one pixel shows up at the edges first.
        for (x, y) in [(0, 0), (19, 0), (0, 19), (19, 19), (10, 10)] {
            assert_close(px(&data, 20, x, y), want, &format!("{what} at {x},{y}"));
        }
    }
}

#[test]
fn a_blit_of_the_whole_canvas_is_the_canvas() {
    // The identity case, which is what a single full-wall output does.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);
    let blit = unmapper_render::Blit::new(&gpu, unmapper_render::TARGET_FORMAT);

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
        virtual_raster: Size::new(16, 16),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    // Top half from the source's top-left (red), bottom half from bottom-left (blue).
    panel(
        &mut show,
        "t",
        Rect::new(0.0, 0.0, 16.0, 8.0),
        "s",
        quadrant(false, false),
    );
    panel(
        &mut show,
        "b",
        Rect::new(0.0, 8.0, 16.0, 8.0),
        "s",
        quadrant(false, true),
    );

    let scene = build_canvas_scene(&show, &textures);
    let canvas = RenderTarget::new(&gpu, show.virtual_raster, "canvas");
    renderer.render_canvas(&gpu, &canvas.view, show.virtual_raster, &scene, &textures);
    let direct = canvas.read_rgba(&gpu);

    let out = RenderTarget::new(&gpu, show.virtual_raster, "output");
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    blit.draw(
        &gpu,
        &mut encoder,
        &out.view,
        &blit.source(&gpu, &canvas.view),
        show.virtual_raster,
        Rect::new(0.0, 0.0, 16.0, 16.0),
    );
    gpu.queue.submit([encoder.finish()]);
    let blitted = out.read_rgba(&gpu);

    assert_eq!(direct, blitted, "a full-canvas blit must not alter a pixel");
    // And it must not be flipped: row 0 is the top panel, which is red.
    assert_close(px(&blitted, 16, 8, 2), RED, "top of the blit");
    assert_close(px(&blitted, 16, 8, 13), BLUE, "bottom of the blit");
}

/// The backdrop is an editing aid, and the single most important thing about it
/// is that it must never reach a monitor standing in for the wall. These check
/// that directly, by rendering both scenes from one show and comparing.
mod backdrop {
    use super::*;
    use unmapper_render::{build_viewport_scene, BACKDROP_ID};

    /// A show with one small panel and a backdrop covering the whole canvas.
    fn backdrop_show(opacity: f32) -> Show {
        let mut show = Show {
            virtual_raster: Size::new(40, 40),
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
        show.geometry.backdrop = Some(unmapper_core::Backdrop {
            path: "mockup.png".into(),
            rect: Rect::new(0.0, 0.0, 40.0, 40.0),
            opacity,
        });
        show
    }

    fn setup(gpu: &Gpu) -> (SourceTextures, Renderer) {
        let mut textures = SourceTextures::new(gpu);
        let renderer = Renderer::new(gpu, &textures.layout);
        // The panel's own content is white, the backdrop is green — so which of
        // the two a pixel came from is unambiguous.
        textures.upload(
            gpu,
            "s",
            FrameUpload {
                width: SRC,
                height: SRC,
                stride: (SRC * 4) as usize,
                bgra: false,
                data: &flat_source(WHITE),
                sequence: 1,
            },
        );
        textures.upload(
            gpu,
            BACKDROP_ID,
            FrameUpload {
                width: SRC,
                height: SRC,
                stride: (SRC * 4) as usize,
                bgra: false,
                data: &flat_source(GREEN),
                sequence: 0,
            },
        );
        (textures, renderer)
    }

    fn render(
        gpu: &Gpu,
        renderer: &mut Renderer,
        textures: &SourceTextures,
        scene: &unmapper_render::Scene,
        size: Size,
    ) -> Vec<u8> {
        let target = RenderTarget::new(gpu, size, "test");
        renderer.render_canvas(gpu, &target.view, size, scene, textures);
        target.read_rgba(gpu)
    }

    #[test]
    fn the_backdrop_shows_in_the_viewport_but_never_on_the_canvas() {
        let gpu = gpu();
        let (textures, mut renderer) = setup(&gpu);
        let show = backdrop_show(1.0);
        let size = show.virtual_raster;

        let viewport = render(
            &gpu,
            &mut renderer,
            &textures,
            &build_viewport_scene(&show, &textures),
            size,
        );
        let canvas = render(
            &gpu,
            &mut renderer,
            &textures,
            &build_canvas_scene(&show, &textures),
            size,
        );

        // Away from the panel, the viewport shows the mockup...
        assert_close(
            px(&viewport, 40, 30, 30),
            GREEN,
            "viewport should show the backdrop",
        );
        // ...and the canvas an output crops from shows nothing at all.
        assert_close(
            px(&canvas, 40, 30, 30),
            [0, 0, 0, 255],
            "the canvas must stay black — a backdrop on the wall would be a bug",
        );

        // The panel itself is identical in both: an aid must not alter content.
        assert_close(px(&viewport, 40, 5, 5), WHITE, "panel in the viewport");
        assert_close(px(&canvas, 40, 5, 5), WHITE, "panel on the canvas");
    }

    #[test]
    fn backdrop_opacity_fades_it_towards_the_background() {
        let gpu = gpu();
        let (textures, mut renderer) = setup(&gpu);
        let show = backdrop_show(0.5);
        let size = show.virtual_raster;

        let viewport = render(
            &gpu,
            &mut renderer,
            &textures,
            &build_viewport_scene(&show, &textures),
            size,
        );

        // Half-opacity green over a black clear is half-brightness green.
        let got = px(&viewport, 40, 30, 30);
        assert!(
            got[1] > 100 && got[1] < 155,
            "expected a half-faded backdrop, got {got:?}"
        );
        assert!(
            got[0] < 10 && got[2] < 10,
            "should still be green, got {got:?}"
        );
    }

    #[test]
    fn a_backdrop_with_no_texture_loaded_is_simply_not_drawn() {
        // The show can name an image before it has been read from disk, or after
        // reading it failed. Neither should leave a hole or a panic.
        let gpu = gpu();
        let textures = SourceTextures::new(&gpu);
        let mut renderer = Renderer::new(&gpu, &textures.layout);
        let show = backdrop_show(1.0);

        let scene = build_viewport_scene(&show, &textures);
        assert!(
            !scene
                .groups
                .iter()
                .any(|g| g.source_id.as_deref() == Some(BACKDROP_ID)),
            "no backdrop group should be emitted without a texture"
        );

        let data = render(&gpu, &mut renderer, &textures, &scene, show.virtual_raster);
        assert_close(px(&data, 40, 30, 30), [0, 0, 0, 255], "should stay black");
    }

    #[test]
    fn the_backdrop_is_drawn_beneath_the_panels() {
        // Order matters: a mockup drawn over the video would hide the thing the
        // operator is trying to place.
        let gpu = gpu();
        let (textures, _) = setup(&gpu);
        let show = backdrop_show(1.0);
        let scene = build_viewport_scene(&show, &textures);

        let backdrop_first = scene
            .groups
            .first()
            .is_some_and(|g| g.source_id.as_deref() == Some(BACKDROP_ID));
        assert!(backdrop_first, "the backdrop must be the first group drawn");
    }
}

/// The set model in the previz view.
mod model {
    use super::*;
    use unmapper_render::{load_gltf, Model};

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn nested_node_transforms_are_baked_into_the_vertices() {
        // The fixture is a unit cube (±1) under a parent translated +2 in X and
        // scaled 2x. A loader that ignored the hierarchy would report ±1.
        let mesh = load_gltf(&fixture("nested-box.gltf")).unwrap();
        let (lo, hi) = mesh.bounds().expect("the cube has vertices");

        assert!(
            (lo.x - 0.0).abs() < 1e-4,
            "min x should be 2-2=0, got {}",
            lo.x
        );
        assert!(
            (hi.x - 4.0).abs() < 1e-4,
            "max x should be 2+2=4, got {}",
            hi.x
        );
        assert!(
            (lo.y + 2.0).abs() < 1e-4,
            "min y should be -2, got {}",
            lo.y
        );
        assert!((hi.y - 2.0).abs() < 1e-4, "max y should be 2, got {}", hi.y);
        assert_eq!(mesh.triangle_count(), 12, "a cube is 12 triangles");
        assert_eq!(mesh.skipped, 0);
    }

    #[test]
    fn a_file_without_normals_gets_face_normals_rather_than_black() {
        // glTF makes NORMAL optional, and plenty of CAD exports omit it. Without
        // a fallback every face would shade to zero.
        let mesh = load_gltf(&fixture("nested-box.gltf")).unwrap();
        assert!(
            mesh.vertices
                .iter()
                .all(|v| glam::Vec3::from(v.normal).length() > 0.5),
            "every vertex should carry a unit-ish normal"
        );
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        assert!(load_gltf(&fixture("does-not-exist.gltf")).is_err());
    }

    #[test]
    fn the_model_renders_and_is_depth_tested_against_the_panels() {
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
                data: &flat_source(RED),
                sequence: 1,
            },
        );

        let mesh = load_gltf(&fixture("nested-box.gltf")).unwrap();
        let model = Model::new(&gpu, &mesh, unmapper_render::DEPTH_FORMAT);
        assert_eq!(model.triangle_count, 12);

        // One panel, upright at the origin, 2m square.
        let mut show = Show {
            virtual_raster: Size::new(64, 64),
            ..Default::default()
        };
        source(&mut show, "s", Size::new(SRC, SRC));
        panel(
            &mut show,
            "p",
            Rect::new(0.0, 0.0, 64.0, 64.0),
            "s",
            quadrant(false, false),
        );
        show.panels[0].placement = unmapper_core::Placement3d {
            translation: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            size: glam::Vec2::new(2.0, 2.0),
        };

        // The model sits at the origin too, but the fixture's geometry spans
        // x 0..4 — so it is off to the panel's right, not in front of it.
        let placement = unmapper_core::Model3d::default();
        let camera = unmapper_core::Camera {
            position: glam::Vec3::new(1.0, 0.0, 9.0),
            target: glam::Vec3::new(1.0, 0.0, 0.0),
            ..Default::default()
        };

        let size = Size::new(96, 96);
        let scene = build_previz_scene(&show, &textures);
        let target = RenderTarget::new(&gpu, size, "previz");
        renderer.render_previz(
            &gpu,
            &target.view,
            size,
            unmapper_render::PrevizView {
                camera: &camera,
                model: Some((&model, &placement)),
            },
            &scene,
            &textures,
        );
        let data = target.read_rgba(&gpu);

        // Somewhere in the frame there must be grey set geometry...
        let has_set = data.as_chunks::<4>().0.iter().any(|p| {
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            r > 20 && (r - g).abs() < 30 && (g - b).abs() < 30 && r < 200
        });
        assert!(has_set, "the set model should be visible");

        // ...and red panel, which the model must not have painted over.
        let has_panel = data
            .as_chunks::<4>().0.iter()
            .any(|p| p[0] > 120 && p[1] < 60 && p[2] < 60);
        assert!(
            has_panel,
            "the panel should still be visible beside the model"
        );
    }

    #[test]
    fn previz_without_a_model_still_renders_the_panels() {
        // The common case: no CAD file loaded at all.
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
                data: &flat_source(RED),
                sequence: 1,
            },
        );

        let mut show = Show {
            virtual_raster: Size::new(64, 64),
            ..Default::default()
        };
        source(&mut show, "s", Size::new(SRC, SRC));
        panel(
            &mut show,
            "p",
            Rect::new(0.0, 0.0, 64.0, 64.0),
            "s",
            quadrant(false, false),
        );
        show.arrange_panels_from_layout();

        let centre = show.panel("p").unwrap().placement.translation;
        let camera = unmapper_core::Camera {
            position: centre + glam::Vec3::new(0.0, 0.0, 3.0),
            target: centre,
            ..Default::default()
        };

        let size = Size::new(64, 64);
        let scene = build_previz_scene(&show, &textures);
        let target = RenderTarget::new(&gpu, size, "previz");
        renderer.render_previz(
            &gpu,
            &target.view,
            size,
            unmapper_render::PrevizView::camera_only(&camera),
            &scene,
            &textures,
        );

        assert_close(
            px(&target.read_rgba(&gpu), 64, 32, 32),
            RED,
            "panel with no model",
        );
    }
}

// --- Warp lattices -------------------------------------------------------
//
// The importer only ever stores a lattice that has actually moved, so the
// first test here is the one that matters most: subdividing must not change
// the picture for a panel that is not really warped.

/// A panel covering `layout`, sampling `src`, optionally through `mesh`.
fn warped_panel(
    show: &mut Show,
    id: &str,
    layout: Rect,
    source_id: &str,
    src: Rect,
    mesh: Option<unmapper_core::WarpMesh>,
) {
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
        source_mesh: mesh,
        slice_id: None,
    });
}

/// Render one 32x32 panel filling the canvas and read it back.
fn render_one(mesh: Option<unmapper_core::WarpMesh>, src: Rect) -> Vec<u8> {
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
        virtual_raster: Size::new(32, 32),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    warped_panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 32.0, 32.0),
        "s",
        src,
        mesh,
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);
    target.read_rgba(&gpu)
}

#[test]
fn an_untouched_lattice_renders_exactly_what_no_lattice_renders() {
    // The whole reason the importer stores `None` for an untouched warper is that
    // the ordinary case must stay what it was. This is that guarantee, checked
    // against the GPU rather than argued from the geometry.
    //
    // The source quad is axis-aligned on purpose: across a *corner-pinned* quad a
    // lattice interpolates bilinearly and the single-quad path interpolates
    // projectively, and those two genuinely differ. Resolume writes the lattice in
    // that case, so its numbers win — but it means the two paths are only required
    // to agree where the quad is a plain rectangle.
    let src = Rect::new(0.0, 0.0, SRC as f32 / 2.0, SRC as f32 / 2.0);
    let plain = render_one(None, src);
    let identity = render_one(
        unmapper_core::WarpMesh::identity(Quad::from_rect(src), 4, 4),
        src,
    );
    assert_eq!(
        plain, identity,
        "subdividing an untouched lattice changed the picture"
    );
}

#[test]
fn a_lattice_decides_the_texels_not_the_output_rect() {
    // The binding's quad names the red top-left quadrant, but the lattice has
    // been slid one quadrant to the right, onto the green one. If the renderer
    // still reads the quad, this comes back red.
    let src = Rect::new(0.0, 0.0, SRC as f32 / 2.0, SRC as f32 / 2.0);
    let slid = unmapper_core::WarpMesh::identity(Quad::from_rect(src), 4, 4)
        .expect("a 4x4 lattice")
        .translate(glam::Vec2::new(SRC as f32 / 2.0, 0.0));

    let data = render_one(Some(slid), src);
    for (x, y) in [(8, 8), (16, 16), (24, 24)] {
        assert_close(
            px(&data, 32, x, y),
            GREEN,
            &format!("lattice sample at {x},{y}"),
        );
    }
}

#[test]
fn a_warped_panel_stays_inside_its_own_layout() {
    // A lattice deforms where the panel *reads from*, never where it *sits*. If a
    // control point could push geometry outside the layout rect, a warped slice
    // would paint over its neighbour on the canvas — and on a monitor standing in
    // for the wall, over a different piece of the rig.
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
            data: &flat_source(WHITE),
            sequence: 1,
        },
    );

    let mut show = Show {
        virtual_raster: Size::new(32, 32),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    // The panel occupies the left half of the canvas only.
    let src = Rect::new(0.0, 0.0, SRC as f32, SRC as f32);
    let mut mesh = unmapper_core::WarpMesh::identity(Quad::from_rect(src), 4, 4).unwrap();
    // Drag control points a long way outside the source, in both directions.
    for p in mesh.points.iter_mut() {
        *p += glam::Vec2::new(400.0, -250.0);
    }
    warped_panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 16.0, 32.0),
        "s",
        src,
        Some(mesh),
    );

    let scene = build_canvas_scene(&show, &textures);
    let target = RenderTarget::new(&gpu, show.virtual_raster, "test");
    renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);

    let data = target.read_rgba(&gpu);
    for y in [2, 16, 30] {
        assert_close(
            px(&data, 32, 24, y),
            [0, 0, 0, 255],
            &format!("canvas right of the panel at y={y} must stay black"),
        );
    }
}

#[test]
fn a_warped_binding_becomes_one_quad_per_cell() {
    let gpu = gpu();
    let textures = SourceTextures::new(&gpu);
    let mut show = Show {
        virtual_raster: Size::new(32, 32),
        ..Default::default()
    };
    source(&mut show, "s", Size::new(SRC, SRC));
    let src = Rect::new(0.0, 0.0, SRC as f32, SRC as f32);
    warped_panel(
        &mut show,
        "p",
        Rect::new(0.0, 0.0, 32.0, 32.0),
        "s",
        src,
        unmapper_core::WarpMesh::identity(Quad::from_rect(src), 4, 5),
    );

    // A 4x5 lattice is a 3x4 grid of cells, two triangles each.
    let scene = build_canvas_scene(&show, &textures);
    assert_eq!(scene.vertices.len(), 3 * 4 * 6);
}

// --- Non-planar surfaces -------------------------------------------------

/// Build a one-panel show, optionally curved, ready for previz.
fn curved_show(surface: unmapper_core::Surface) -> Show {
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
    show.panels[0].surface = surface;
    show
}

#[test]
fn the_emulation_canvas_ignores_a_curved_surface() {
    // The invariant that protects the whole emulation path: one canvas pixel is
    // one LED, and an output crops the canvas to a monitor standing in for a
    // piece of the rig. A curved *stage* surface must not bend that, or every
    // stand-in monitor stops being pixel-exact the moment someone curves a wall.
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

    let mut render = |surface: unmapper_core::Surface| {
        let show = curved_show(surface);
        let scene = build_canvas_scene(&show, &textures);
        let target = RenderTarget::new(&gpu, show.virtual_raster, "canvas");
        renderer.render_canvas(&gpu, &target.view, show.virtual_raster, &scene, &textures);
        (target.read_rgba(&gpu), scene.vertices.len())
    };

    let (flat, flat_verts) = render(unmapper_core::Surface::Flat);
    let (curved, curved_verts) = render(unmapper_core::Surface::Arc { sweep_deg: 120.0 });
    assert_eq!(flat, curved, "a curved surface changed the emulation canvas");
    assert_eq!(flat_verts, 6, "a flat panel is still two triangles");
    assert_eq!(
        curved_verts, 6,
        "the canvas must not subdivide for a stage surface it does not use"
    );
}

#[test]
fn a_curved_panel_bends_in_previz_and_a_flat_one_does_not() {
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
            data: &flat_source(WHITE),
            sequence: 1,
        },
    );

    let mut render = |surface: unmapper_core::Surface| {
        let show = curved_show(surface);
        let centre = show.panel("p").unwrap().placement.translation;
        // Look down on the panel from above and in front, so a curve towards or
        // away from the audience actually changes the silhouette. Straight on, a
        // symmetric arc would look almost like a flat panel.
        let camera = unmapper_core::Camera {
            position: centre + glam::Vec3::new(0.0, 3.0, 3.0),
            target: centre,
            ..Default::default()
        };
        let size = Size::new(64, 64);
        let scene = build_previz_scene(&show, &textures);
        let target = RenderTarget::new(&gpu, size, "previz");
        renderer.render_previz(
            &gpu,
            &target.view,
            size,
            unmapper_render::PrevizView::camera_only(&camera),
            &scene,
            &textures,
        );
        let data = target.read_rgba(&gpu);
        let lit = data
            .as_chunks::<4>().0.iter()
            .filter(|p| p[0] > 40 || p[1] > 40 || p[2] > 40)
            .count();
        (lit, scene.vertices.len())
    };

    let (flat_lit, flat_verts) = render(unmapper_core::Surface::Flat);
    let (curved_lit, curved_verts) = render(unmapper_core::Surface::Arc { sweep_deg: 120.0 });

    assert_eq!(flat_verts, 6, "a flat panel is still two triangles");
    // 120 degrees at 5 per segment is 24 cells across, one down.
    assert_eq!(curved_verts, 24 * 6, "the arc should subdivide across only");

    assert!(flat_lit > 0 && curved_lit > 0, "both should draw something");
    assert!(
        (flat_lit as i64 - curved_lit as i64).abs() > flat_lit as i64 / 20,
        "curving the panel barely changed the image: flat lit {flat_lit}, curved {curved_lit}"
    );
}

#[test]
fn a_warped_slice_on_a_curved_panel_uses_one_grid_for_both() {
    // The two subdivisions are independent and have to share one grid: the warp
    // lattice on the source side, the arc on the destination side.
    let gpu = gpu();
    let mut textures = SourceTextures::new(&gpu);
    textures.upload(
        &gpu,
        "s",
        FrameUpload {
            width: SRC,
            height: SRC,
            stride: (SRC * 4) as usize,
            bgra: false,
            data: &flat_source(WHITE),
            sequence: 1,
        },
    );
    let src = quadrant(false, false);

    let cells = |sweep_deg: f32, columns: u32, rows: u32| {
        let mut show = curved_show(unmapper_core::Surface::Arc { sweep_deg });
        show.bindings[0].source_mesh =
            unmapper_core::WarpMesh::identity(Quad::from_rect(src), columns, rows);
        build_previz_scene(&show, &textures).vertices.len() / 6
    };

    // 120 degrees is 24 cells across; a 4x4 lattice is 3. 3 divides 24, so the
    // shared grid is 24 across and the lattice's creases still land on cell edges.
    assert_eq!(cells(120.0, 4, 4), 24 * 3);

    // 25 degrees is 5 across, which 3 does *not* divide. Taking the larger of the
    // two would give 5 and drop a crease into the middle of a cell; the common
    // refinement is 15.
    assert_eq!(cells(25.0, 4, 2), 15);

    // Neither one subdividing is still the plain two triangles.
    let flat = curved_show(unmapper_core::Surface::Flat);
    assert_eq!(build_previz_scene(&flat, &textures).vertices.len(), 6);
}
