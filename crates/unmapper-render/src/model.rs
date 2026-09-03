//! Loading a set model into the previz view.
//!
//! Reads glTF 2.0 (`.gltf` + buffers, or a self-contained `.glb`), which is what
//! every CAD and 3D tool in this world exports — Blender, Cinema 4D, SketchUp,
//! Vectorworks via an exporter. Only geometry is read: positions, normals and
//! indices. Materials, textures, cameras and animation are ignored on purpose.
//! This model is context for judging where the walls sit, not a render.
//!
//! # Node transforms are baked, not kept
//!
//! A real export nests geometry under a hierarchy of transformed nodes — a truss
//! rotated inside a rig group inside the scene root. Rather than carrying that
//! tree onto the GPU, the scene graph is walked once at load and each node's
//! world transform is baked into its vertices. The result is one flat buffer,
//! one draw, and no per-frame hierarchy work; the cost is that the model cannot
//! be re-articulated afterwards, which nothing here wants to do.

use std::path::Path;

use anyhow::{Context, Result};
use unmapper_core::Model3d;

use crate::gpu::Gpu;

/// One vertex of set geometry.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl ModelVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ModelVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
    };
}

/// Geometry on the CPU, before it reaches the GPU.
#[derive(Debug, Default, Clone)]
pub struct MeshData {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    /// How many primitives were skipped for having no position data, so the UI
    /// can say so rather than silently showing less than the file contains.
    pub skipped: usize,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The axis-aligned bounds, for framing a camera on the model.
    pub fn bounds(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        let mut it = self.vertices.iter().map(|v| glam::Vec3::from(v.position));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
}

/// Read a glTF or GLB file into one flat mesh.
pub fn load_gltf(path: &Path) -> Result<MeshData> {
    let (document, buffers, _images) =
        gltf::import(path).with_context(|| format!("reading {}", path.display()))?;

    let mut out = MeshData::default();

    // Walk every scene's node tree, accumulating transforms. A file with no
    // declared default scene still has nodes worth drawing, so all scenes are
    // walked rather than just `document.default_scene()`.
    for scene in document.scenes() {
        for node in scene.nodes() {
            walk(&node, glam::Mat4::IDENTITY, &buffers, &mut out);
        }
    }

    if out.is_empty() && out.skipped == 0 {
        anyhow::bail!(
            "{} contains no geometry — it may be a scene of cameras or lights only",
            path.display()
        );
    }
    Ok(out)
}

fn walk(node: &gltf::Node, parent: glam::Mat4, buffers: &[gltf::buffer::Data], out: &mut MeshData) {
    let local = glam::Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;

    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            append_primitive(&primitive, world, buffers, out);
        }
    }

    for child in node.children() {
        walk(&child, world, buffers, out);
    }
}

fn append_primitive(
    primitive: &gltf::Primitive,
    world: glam::Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut MeshData,
) {
    // Only triangles. Points and lines have no surface to shade, and a CAD
    // export that is all lines would be better handled as its own thing.
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        out.skipped += 1;
        return;
    }

    let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let Some(positions) = reader.read_positions() else {
        out.skipped += 1;
        return;
    };
    let positions: Vec<[f32; 3]> = positions.collect();

    // Normals are optional in glTF. Without them a flat face would shade black,
    // so a face normal is computed per triangle further down; here they default
    // to zero and are filled in after.
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(n) => n.collect(),
        None => vec![[0.0, 0.0, 0.0]; positions.len()],
    };

    let base = out.vertices.len() as u32;
    let normal_matrix = glam::Mat3::from_mat4(world);

    for (i, p) in positions.iter().enumerate() {
        let world_pos = world.transform_point3(glam::Vec3::from(*p));
        let n = normals.get(i).copied().unwrap_or([0.0; 3]);
        let world_normal = normal_matrix * glam::Vec3::from(n);
        out.vertices.push(ModelVertex {
            position: world_pos.to_array(),
            normal: world_normal.normalize_or_zero().to_array(),
        });
    }

    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        // No index buffer means the vertices are already in draw order.
        None => (0..positions.len() as u32).collect(),
    };

    // Fill in any missing normals from the triangle they belong to, so a file
    // exported without normals still shades rather than rendering flat black.
    let had_normals = reader.read_normals().is_some();
    if !had_normals {
        for tri in indices.chunks_exact(3) {
            let (a, b, c) = (
                base as usize + tri[0] as usize,
                base as usize + tri[1] as usize,
                base as usize + tri[2] as usize,
            );
            let (pa, pb, pc) = (
                glam::Vec3::from(out.vertices[a].position),
                glam::Vec3::from(out.vertices[b].position),
                glam::Vec3::from(out.vertices[c].position),
            );
            let face = (pb - pa).cross(pc - pa).normalize_or_zero();
            for v in [a, b, c] {
                out.vertices[v].normal = face.to_array();
            }
        }
    }

    out.indices.extend(indices.into_iter().map(|i| i + base));
}

/// Set geometry on the GPU, plus the pipeline that draws it.
pub struct Model {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    pub triangle_count: usize,
    pub skipped: usize,
}

/// Must match `ModelGlobals` in `model.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ModelGlobals {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    tint: [f32; 4],
}

impl Model {
    pub fn new(gpu: &Gpu, mesh: &MeshData, depth_format: wgpu::TextureFormat) -> Self {
        use wgpu::util::DeviceExt as _;

        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("model"),
                source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
            });

        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("model globals"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let globals = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model globals"),
            size: std::mem::size_of::<ModelGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        // An empty buffer is invalid, so a model with no geometry still gets one
        // vertex; index_count of 0 means it is never drawn.
        let vertices = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model vertices"),
                contents: if mesh.vertices.is_empty() {
                    bytemuck::cast_slice(&[ModelVertex {
                        position: [0.0; 3],
                        normal: [0.0, 1.0, 0.0],
                    }])
                } else {
                    bytemuck::cast_slice(&mesh.vertices)
                },
                usage: wgpu::BufferUsages::VERTEX,
            });

        let indices = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model indices"),
                contents: if mesh.indices.is_empty() {
                    bytemuck::cast_slice(&[0u32])
                } else {
                    bytemuck::cast_slice(&mesh.indices)
                },
                usage: wgpu::BufferUsages::INDEX,
            });

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("model"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });

        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("model"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[Some(ModelVertex::LAYOUT)],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: crate::TARGET_FORMAT,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Set models are routinely modelled single-sided and viewed
                    // from behind; culling would make walls vanish.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });

        Self {
            pipeline,
            globals,
            bind_group,
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            triangle_count: mesh.triangle_count(),
            skipped: mesh.skipped,
        }
    }

    /// The model matrix implied by a [`Model3d`]'s scale, rotation and position.
    pub fn transform(placement: &Model3d) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::splat(placement.scale.max(1e-6)),
            placement.rotation,
            placement.translation,
        )
    }

    /// Draw into an existing pass. The caller owns the clear and the depth
    /// buffer, so the model and the panels share one depth test and occlude each
    /// other correctly.
    pub fn draw(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        view_proj: glam::Mat4,
        placement: &Model3d,
        tint: [f32; 4],
    ) {
        if self.index_count == 0 {
            return;
        }
        gpu.queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&ModelGlobals {
                view_proj: view_proj.to_cols_array_2d(),
                model: Self::transform(placement).to_cols_array_2d(),
                tint,
            }),
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_block_is_std140_compatible() {
        // Two mat4s then a vec4, all 16-byte aligned already.
        assert_eq!(std::mem::size_of::<ModelGlobals>(), 64 + 64 + 16);
    }

    #[test]
    fn vertex_stride_matches_its_attributes() {
        assert_eq!(std::mem::size_of::<ModelVertex>(), 6 * 4);
        assert_eq!(ModelVertex::LAYOUT.array_stride, 24);
    }

    #[test]
    fn bounds_of_an_empty_mesh_are_none_rather_than_infinite() {
        assert!(MeshData::default().bounds().is_none());
    }

    #[test]
    fn the_transform_applies_scale_rotation_and_position() {
        let placement = Model3d {
            scale: 2.0,
            translation: glam::Vec3::new(1.0, 0.0, 0.0),
            rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        };
        let m = Model::transform(&placement);
        // A point one unit along +Z, doubled and turned 90 degrees about Y,
        // lands two units along +X, then shifts by the translation.
        let p = m.transform_point3(glam::Vec3::Z);
        assert!(
            (p - glam::Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4,
            "got {p:?}"
        );
    }

    #[test]
    fn a_zero_scale_does_not_collapse_the_model_to_a_point() {
        // A hand-edited stage file can easily carry scale="0"; clamping keeps the
        // matrix invertible and the model merely tiny rather than gone.
        let placement = Model3d {
            scale: 0.0,
            ..Default::default()
        };
        let m = Model::transform(&placement);
        assert!(m.determinant().abs() > 0.0);
    }
}
