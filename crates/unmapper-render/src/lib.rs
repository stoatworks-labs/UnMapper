//! GPU rendering for UnMapper.
//!
//! Two views of one stage, sharing one shader and one set of source textures:
//!
//! - **Emulation** ([`Renderer::render_canvas`]) draws every panel at its 2D
//!   layout on the virtual raster, orthographically and pixel-exact. An output
//!   then crops a region of that canvas to a monitor, so a grid of monitors
//!   stands in for the real wall.
//! - **Previz** ([`Renderer::render_previz`]) draws the same panels at their 3D
//!   poses, through a camera.
//!
//! Both consume the same [`SourceTextures`], so a frame is uploaded once however
//! many outputs display it.

pub mod blit;
pub mod gpu;
pub mod model;
pub mod offline;
pub mod pattern;
pub mod source;

use std::ops::Range;

use unmapper_core::{Camera, Quad, Rect, Show, Size, Vec2};

pub use blit::Blit;
pub use gpu::{Globals, Gpu, Vertex, TARGET_FORMAT};
pub use model::{load_gltf, MeshData, Model, ModelVertex};
pub use offline::{load_image, sync_offline_sources};
pub use pattern::test_pattern;
pub use source::{FrameUpload, SourceTexture, SourceTextures};

/// Drawn for a panel whose source has no frame yet — dim, so it reads as "wired
/// up but silent" rather than as either black (broken) or bright (live).
const UNBOUND_TINT: [f32; 4] = [0.10, 0.10, 0.12, 1.0];
/// The set model's base colour — a neutral grey that reads as scenery and does
/// not compete with the video on the walls.
const SET_TINT: [f32; 4] = [0.42, 0.44, 0.48, 1.0];
const LIVE_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// How much of the emulation canvas a viewport shows.
///
/// `pan` is the canvas-space point at the viewport's top-left; `zoom` is viewport
/// pixels per canvas pixel. Rendering the whole canvas 1:1 is [`CanvasView::FULL`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasView {
    pub pan: Vec2,
    pub zoom: f32,
}

impl CanvasView {
    /// The whole canvas at one texel per canvas pixel.
    pub const FULL: Self = Self {
        pan: Vec2::ZERO,
        zoom: 1.0,
    };

    pub fn new(pan: Vec2, zoom: f32) -> Self {
        Self { pan, zoom }
    }
}

/// How the 3D view is set up: where it is looked at from, and what set geometry
/// is behind the panels.
///
/// Grouped because they always travel together — a camera with no set and a set
/// with no camera are both meaningless.
pub struct PrevizView<'a> {
    pub camera: &'a Camera,
    /// The set model and its placement, if one is loaded.
    pub model: Option<(&'a Model, &'a unmapper_core::Model3d)>,
}

impl<'a> PrevizView<'a> {
    /// A camera with no set geometry — the common case before a CAD file is
    /// loaded, and what the emulation-only workflow uses.
    pub fn camera_only(camera: &'a Camera) -> Self {
        Self {
            camera,
            model: None,
        }
    }
}

/// A run of vertices sharing one source texture.
pub struct DrawGroup {
    /// `None` means draw with the placeholder texture and the unbound tint.
    pub source_id: Option<String>,
    pub range: Range<u32>,
}

/// Vertices plus the groups indexing into them.
#[derive(Default)]
pub struct Scene {
    pub vertices: Vec<Vertex>,
    pub groups: Vec<DrawGroup>,
}

impl Scene {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

/// Emit the two triangles for one quad.
///
/// The winding is tl→tr→br, tl→br→bl. Both triangles carry the same projective
/// weights, which is what keeps them agreeing across the shared diagonal.
fn push_quad(
    out: &mut Vec<Vertex>,
    dest: [glam::Vec3; 4],
    src_uv: Quad,
    weights: [f32; 4],
    tint: [f32; 4],
) {
    let uvq = |i: usize| {
        let uv = match i {
            0 => src_uv.tl,
            1 => src_uv.tr,
            2 => src_uv.br,
            _ => src_uv.bl,
        };
        let q = weights[i];
        [uv.x * q, uv.y * q, q]
    };
    let vert = |i: usize| Vertex {
        position: dest[i].to_array(),
        uvq: uvq(i),
        tint,
    };
    for i in [0usize, 1, 2, 0, 2, 3] {
        out.push(vert(i));
    }
}

/// Bilinear point on a destination quad, wound tl, tr, br, bl.
///
/// The emulation canvas is always a plain rect, so bilinear is exact there. The
/// previz side does not use this — it asks the panel's [`unmapper_core::Surface`]
/// instead, which is the whole point of surfaces.
fn quad_point(dest: [glam::Vec3; 4], u: f32, v: f32) -> glam::Vec3 {
    let top = dest[0].lerp(dest[1], u);
    let bottom = dest[3].lerp(dest[2], u);
    top.lerp(bottom, v)
}

/// The grid resolution that serves two subdivisions at once.
///
/// The lowest common multiple, so every boundary of *both* grids lands on a cell
/// edge. Taking the larger of the two instead would let a warp crease fall in the
/// middle of a render cell, where bilinear interpolation quietly smooths away the
/// fold the operator put there. Capped, because an awkward pair (say 18 and 25)
/// multiplies out to a mesh nobody asked for.
fn refine(a: u32, b: u32) -> u32 {
    const CAP: u32 = 256;
    let (a, b) = (a.max(1), b.max(1));
    let mut x = a;
    let mut y = b;
    while y != 0 {
        let t = x % y;
        x = y;
        y = t;
    }
    (a / x).saturating_mul(b).clamp(1, CAP)
}

/// How a panel's destination geometry is found, which is the only thing the two
/// views disagree about.
#[derive(Clone, Copy)]
enum View {
    /// The emulation canvas: the panel's 2D layout rect, always flat.
    ///
    /// A curved surface deliberately does **not** deform this. One canvas pixel
    /// is one LED and an output crops the canvas to a monitor standing in for a
    /// piece of the rig; bending the canvas would break both.
    Canvas,
    /// The 3D stage, through the panel's surface.
    Previz,
}

/// The reserved source id the backdrop image is uploaded under.
///
/// It shares [`SourceTextures`] with the NDI feeds rather than living in its own
/// slot, so the one texture-binding path serves both. The double underscore
/// keeps it clear of anything an imported slice map could produce.
pub const BACKDROP_ID: &str = "__backdrop";

/// Build the scene that goes to **outputs**: every enabled panel at its 2D
/// layout, and nothing else.
///
/// Deliberately excludes the backdrop and any other editing aid. This is the
/// image a monitor standing in for the wall will show, so anything in here is
/// something the audience would see on the real LED.
pub fn build_canvas_scene(show: &Show, textures: &SourceTextures) -> Scene {
    let mut scene = Scene::default();
    append_panels(&mut scene, show, textures, View::Canvas);
    scene
}

/// Build the scene for the **editing viewport**: the backdrop mockup first, then
/// the panels over it.
///
/// The viewport renders separately from the canvas rather than cropping it,
/// precisely so aids like this can exist without any risk of reaching an output.
pub fn build_viewport_scene(show: &Show, textures: &SourceTextures) -> Scene {
    let mut scene = Scene::default();
    append_backdrop(&mut scene, show, textures);
    append_panels(&mut scene, show, textures, View::Canvas);
    scene
}

/// Build the previz scene: every enabled panel at its 3D pose.
pub fn build_previz_scene(show: &Show, textures: &SourceTextures) -> Scene {
    let mut scene = Scene::default();
    append_panels(&mut scene, show, textures, View::Previz);
    scene
}

fn panel_layout_quad(panel: &unmapper_core::Panel) -> [glam::Vec3; 4] {
    let r = panel.layout;
    [
        glam::Vec3::new(r.x, r.y, 0.0),
        glam::Vec3::new(r.right(), r.y, 0.0),
        glam::Vec3::new(r.right(), r.bottom(), 0.0),
        glam::Vec3::new(r.x, r.bottom(), 0.0),
    ]
}

/// The backdrop image as one quad in canvas space, drawn before the panels so
/// they sit on top of it.
fn append_backdrop(scene: &mut Scene, show: &Show, textures: &SourceTextures) {
    let Some(backdrop) = &show.geometry.backdrop else {
        return;
    };
    if textures.get(BACKDROP_ID).is_none() || backdrop.rect.is_empty() {
        return;
    }

    let start = scene.vertices.len() as u32;
    let r = backdrop.rect;
    push_quad(
        &mut scene.vertices,
        [
            glam::Vec3::new(r.x, r.y, 0.0),
            glam::Vec3::new(r.right(), r.y, 0.0),
            glam::Vec3::new(r.right(), r.bottom(), 0.0),
            glam::Vec3::new(r.x, r.bottom(), 0.0),
        ],
        Quad::from_rect(Rect::from_size(1.0, 1.0)),
        [1.0; 4],
        // Opacity rides in the tint's alpha; the pipeline already alpha-blends,
        // so a faded mockup costs nothing extra.
        [1.0, 1.0, 1.0, backdrop.opacity.clamp(0.0, 1.0)],
    );
    scene.groups.push(DrawGroup {
        source_id: Some(BACKDROP_ID.to_owned()),
        range: start..scene.vertices.len() as u32,
    });
}

fn append_panels(scene: &mut Scene, show: &Show, textures: &SourceTextures, view: View) {
    // Group by source so consecutive panels from one Resolume output share a
    // bind group and a draw call boundary.
    let mut ordered: Vec<&unmapper_core::Binding> = show.bindings.iter().collect();
    ordered.sort_by(|a, b| a.source_id.cmp(&b.source_id));

    let mut current: Option<(Option<String>, u32)> = None;

    for binding in ordered {
        let Some(panel) = show.panel(&binding.panel_id) else {
            continue;
        };
        if !panel.enabled {
            continue;
        }

        let expected = show.source(&binding.source_id).and_then(|s| s.expected);
        let live = textures.source_size(&binding.source_id, expected);
        let has_frame = textures.get(&binding.source_id).is_some();

        // With no size to divide by there is no way to turn the pixel-space quad
        // into texture coordinates, so fall back to the whole texture rather than
        // dividing by zero.
        let source_size = match live {
            Some(size) if size.width > 0 && size.height > 0 => Some(size.as_vec2()),
            _ => None,
        };
        let src_uv = match source_size {
            Some(size) => binding.source_quad.to_uv(size),
            None => Quad::from_rect(Rect::from_size(1.0, 1.0)),
        };

        let key = has_frame.then(|| binding.source_id.clone());
        let start = scene.vertices.len() as u32;

        match &mut current {
            Some((k, _)) if *k == key => {}
            _ => {
                if let Some((k, s)) = current.take() {
                    scene.groups.push(DrawGroup {
                        source_id: k,
                        range: s..start,
                    });
                }
                current = Some((key.clone(), start));
            }
        }

        let tint = if has_frame { LIVE_TINT } else { UNBOUND_TINT };
        let layout_quad = panel_layout_quad(panel);

        // Two things can subdivide a panel, and they are independent: the warp
        // lattice on the source side, and a non-flat surface on the destination
        // side. Both are evaluated on one shared grid.
        //
        // An ordinary panel — flat surface, untouched warper — comes out of this
        // as a 1x1 grid whose single cell is the panel's own quad and the
        // binding's own quad, so it is still two triangles and still bit-for-bit
        // what it was before any of this existed. The importer storing `None` for
        // Arena's untouched warpers is what keeps it on that path. Without a frame
        // size there are no texture coordinates for the cells either, so that case
        // also falls back to the plain quad.
        let (surf_u, surf_v) = match view {
            View::Canvas => (1, 1),
            View::Previz => panel.subdivisions(),
        };
        let warp = source_size.and(binding.source_mesh.as_ref());
        let (warp_u, warp_v) = warp.map(|m| m.cell_counts()).unwrap_or((1, 1));
        let (nu, nv) = (refine(surf_u, warp_u), refine(surf_v, warp_v));

        let dest_at = |u: f32, v: f32| match view {
            View::Canvas => quad_point(layout_quad, u, v),
            View::Previz => panel.surface_point(u, v),
        };
        // With no lattice the source is the binding's quad, evaluated
        // projectively — `lerp` here would be the bilinear bug that
        // `Quad::project` exists to avoid, reintroduced per cell.
        let source_at = |u: f32, v: f32| match warp {
            Some(mesh) => mesh.source_at(u, v),
            None => binding.source_quad.project(u, v),
        };

        for row in 0..nv {
            for col in 0..nu {
                let u0 = col as f32 / nu as f32;
                let u1 = (col + 1) as f32 / nu as f32;
                let v0 = row as f32 / nv as f32;
                let v1 = (row + 1) as f32 / nv as f32;

                let cell_dest = [
                    dest_at(u0, v0),
                    dest_at(u1, v0),
                    dest_at(u1, v1),
                    dest_at(u0, v1),
                ];
                let cell_src = Quad::new(
                    source_at(u0, v0),
                    source_at(u1, v0),
                    source_at(u1, v1),
                    source_at(u0, v1),
                );
                let cell_uv = match source_size {
                    Some(size) => cell_src.to_uv(size),
                    None => src_uv,
                };
                // Per-cell weights. One set taken across the whole panel would be
                // the bilinear map, cell by cell.
                push_quad(
                    &mut scene.vertices,
                    cell_dest,
                    cell_uv,
                    cell_src.projective_weights(),
                    tint,
                );
            }
        }
    }

    if let Some((k, s)) = current {
        let end = scene.vertices.len() as u32;
        if end > s {
            scene.groups.push(DrawGroup {
                source_id: k,
                range: s..end,
            });
        }
    }
}

/// The pipelines and buffers. One per application.
pub struct Renderer {
    canvas_pipeline: wgpu::RenderPipeline,
    previz_pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    depth: Option<(wgpu::TextureView, u32, u32)>,
}

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl Renderer {
    pub fn new(gpu: &Gpu, source_layout: &wgpu::BindGroupLayout) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("panel"),
                source: wgpu::ShaderSource::Wgsl(include_str!("panel.wgsl").into()),
            });

        let globals_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("globals"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });

        let globals_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("panel"),
                bind_group_layouts: &[Some(&globals_layout), Some(source_layout)],
                immediate_size: 0,
            });

        let make = |label: &str, vs: &str, depth: bool| {
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        buffers: &[Vertex::LAYOUT],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: TARGET_FORMAT,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        // Panels are viewed from both sides in previz — walking
                        // behind the wall is a legitimate thing to do with a
                        // previz camera — so nothing is culled.
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: depth.then(|| wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                })
        };

        Self {
            canvas_pipeline: make("canvas", "vs_canvas", false),
            previz_pipeline: make("previz", "vs_previz", true),
            globals_buffer,
            globals_bind_group,
            vertex_buffer: gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("panels"),
                size: 1024,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vertex_capacity: 1024 / std::mem::size_of::<Vertex>(),
            depth: None,
        }
    }

    fn ensure_vertex_capacity(&mut self, gpu: &Gpu, needed: usize) {
        if needed <= self.vertex_capacity {
            return;
        }
        let capacity = needed.next_power_of_two();
        self.vertex_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("panels"),
            size: (capacity * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_capacity = capacity;
    }

    fn ensure_depth(&mut self, gpu: &Gpu, width: u32, height: u32) {
        if matches!(self.depth, Some((_, w, h)) if w == width && h == height) {
            return;
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("previz depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth = Some((
            texture.create_view(&wgpu::TextureViewDescriptor::default()),
            width.max(1),
            height.max(1),
        ));
    }

    /// Draw the whole emulation canvas at 1:1 — one canvas pixel per texel.
    ///
    /// This is the pixel-exact path an output crops from, and what the CLI writes
    /// to a PNG. For an interactive, scrollable view use [`Renderer::render_canvas_view`].
    pub fn render_canvas(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        canvas: Size,
        scene: &Scene,
        textures: &SourceTextures,
    ) {
        self.render_canvas_panned(gpu, target, canvas, CanvasView::FULL, scene, textures);
    }

    /// Draw the emulation canvas into a viewport, scrolled and scaled.
    ///
    /// Geometry is untouched — the mapping happens in the vertex shader — so
    /// panning and zooming are free.
    pub fn render_canvas_view(
        &mut self,
        gpu: &Gpu,
        target: &RenderTarget,
        view: CanvasView,
        scene: &Scene,
        textures: &SourceTextures,
    ) {
        self.render_canvas_panned(gpu, &target.view, target.size, view, scene, textures);
    }

    fn render_canvas_panned(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        viewport: Size,
        view: CanvasView,
        scene: &Scene,
        textures: &SourceTextures,
    ) {
        gpu.queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::bytes_of(&Globals {
                viewport: [viewport.width.max(1) as f32, viewport.height.max(1) as f32],
                pan: [view.pan.x, view.pan.y],
                // A zero or negative zoom would collapse every panel to a point;
                // clamp rather than trusting a UI that might briefly produce one.
                zoom: view.zoom.max(1e-4),
                ..Default::default()
            }),
        );
        self.draw(gpu, target, scene, textures, false);
    }

    /// Draw the 3D previz view.
    ///
    /// `model` is the set geometry, if one is loaded. It shares the pass and the
    /// depth buffer with the panels, so a wall behind a truss is correctly hidden
    /// by it rather than drawn over the top.
    pub fn render_previz(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        size: Size,
        view: PrevizView<'_>,
        scene: &Scene,
        textures: &SourceTextures,
    ) {
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        let view_proj = view.camera.view_projection(aspect);
        gpu.queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::bytes_of(&Globals {
                viewport: [size.width as f32, size.height as f32],
                view_proj: view_proj.to_cols_array_2d(),
                ..Default::default()
            }),
        );
        self.ensure_depth(gpu, size.width, size.height);
        self.upload_scene(gpu, scene);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("previz"),
            });
        {
            let (depth_view, _, _) = self.depth.as_ref().expect("depth ensured above");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("previz"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Set first, panels over it — both depth-tested, so the order is a
            // hint rather than what decides occlusion.
            if let Some((model, placement)) = view.model {
                model.draw(gpu, &mut pass, view_proj, placement, SET_TINT);
            }
            self.draw_panels(&mut pass, scene, textures, true);
        }
        gpu.queue.submit([encoder.finish()]);
    }

    /// Upload this frame's geometry. Must happen before any pass that draws it.
    fn upload_scene(&mut self, gpu: &Gpu, scene: &Scene) {
        self.ensure_vertex_capacity(gpu, scene.vertices.len().max(1));
        if !scene.vertices.is_empty() {
            gpu.queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&scene.vertices),
            );
        }
    }

    /// Draw the panels into a pass the caller owns.
    ///
    /// Separate from [`Renderer::draw`] so the previz view can put the set model
    /// and the panels in **one** pass, sharing a depth buffer — which is what
    /// makes a wall behind a truss correctly hidden by it.
    fn draw_panels(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        scene: &Scene,
        textures: &SourceTextures,
        depth: bool,
    ) {
        pass.set_pipeline(if depth {
            &self.previz_pipeline
        } else {
            &self.canvas_pipeline
        });
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

        for group in &scene.groups {
            let texture = group
                .source_id
                .as_deref()
                .and_then(|id| textures.get(id))
                .unwrap_or(&textures.placeholder);
            pass.set_bind_group(1, &texture.bind_group, &[]);
            pass.draw(group.range.clone(), 0..1);
        }
    }

    fn draw(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        scene: &Scene,
        textures: &SourceTextures,
        depth: bool,
    ) {
        self.upload_scene(gpu, scene);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("draw"),
            });

        {
            let depth_attachment = depth.then(|| {
                let (view, _, _) = self.depth.as_ref().expect("depth ensured before draw");
                wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }
            });

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("panels"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: depth_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.draw_panels(&mut pass, scene, textures, depth);
        }

        gpu.queue.submit([encoder.finish()]);
    }
}

/// An offscreen colour target that can be read back or handed to a texture-share
/// publisher.
pub struct RenderTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: Size,
}

impl RenderTarget {
    pub fn new(gpu: &Gpu, size: Size, label: &str) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size,
        }
    }

    /// Copy the target back to the CPU as tightly packed RGBA.
    ///
    /// This is what feeds NDI output and what the tests assert on. It is a real
    /// stall — the GPU has to finish before the bytes exist — so it belongs on
    /// the output path and in tests, not in the middle of a frame.
    pub fn read_rgba(&self, gpu: &Gpu) -> Vec<u8> {
        // A buffer copy needs each row padded to 256 bytes, unlike write_texture.
        let unpadded = self.size.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;

        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * self.size.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.size.height),
                },
            },
            wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        let mapped = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded * self.size.height) as usize);
        for row in 0..self.size.height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        out
    }
}

/// The sub-rectangle of the canvas one output shows, in texture coordinates.
///
/// Emulation outputs crop the canvas rather than re-rendering it, so N monitors
/// showing N pieces of one wall cost one render and N blits.
pub fn crop_uv(canvas: Size, region: Rect) -> Quad {
    let size = Vec2::new(canvas.width.max(1) as f32, canvas.height.max(1) as f32);
    Quad::from_rect(region).to_uv(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unmapper_core::{Panel, Rect};

    fn show_with_one_panel() -> Show {
        let mut show = Show {
            virtual_raster: Size::new(200, 100),
            ..Default::default()
        };
        show.sources.push(unmapper_core::Source {
            id: "s".into(),
            name: "s".into(),
            kind: unmapper_core::SourceKind::TestPattern,
            space: unmapper_core::SourceSpace::Composition,
            expected: Some(Size::new(100, 100)),
            enabled: true,
        });
        show.panels.push(Panel::from_layout(
            "p",
            "P",
            Size::new(100, 100),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            2.6,
        ));
        show.bindings.push(unmapper_core::Binding {
            panel_id: "p".into(),
            source_id: "s".into(),
            source_quad: Quad::from_rect(Rect::from_size(100.0, 100.0)),
            source_mesh: None,
            slice_id: None,
        });
        show
    }

    #[test]
    fn a_panel_becomes_two_triangles() {
        let gpu = Gpu::new_blocking().expect("a GPU");
        let textures = SourceTextures::new(&gpu);
        let scene = build_canvas_scene(&show_with_one_panel(), &textures);
        assert_eq!(scene.vertices.len(), 6);
        assert_eq!(scene.groups.len(), 1);
        // No frame uploaded yet, so it draws with the placeholder.
        assert_eq!(scene.groups[0].source_id, None);
    }

    #[test]
    fn a_disabled_panel_is_not_drawn() {
        let gpu = Gpu::new_blocking().expect("a GPU");
        let textures = SourceTextures::new(&gpu);
        let mut show = show_with_one_panel();
        show.panels[0].enabled = false;
        assert!(build_canvas_scene(&show, &textures).is_empty());
    }

    #[test]
    fn crop_uv_maps_a_region_to_zero_one() {
        let uv = crop_uv(Size::new(1000, 500), Rect::new(500.0, 0.0, 500.0, 250.0));
        assert_eq!(uv.tl, Vec2::new(0.5, 0.0));
        assert_eq!(uv.br, Vec2::new(1.0, 0.5));
    }
}
