//! Copying a region of the canvas onto an output.

use unmapper_core::{Quad, Rect, Size};

use crate::gpu::Gpu;

/// The source rectangle, in 0..1 texture coordinates.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlitUniform {
    src_rect: [f32; 4],
}

/// A pass that draws a sub-rectangle of one texture across a whole target.
///
/// Built once and reused for every output; the source texture and region change
/// per draw, the pipeline does not.
pub struct Blit {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// Layout for the source texture + sampler pair, so callers can build a bind
    /// group for whatever they want to blit.
    pub source_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

impl Blit {
    /// `format` must match the target being drawn into — a surface's format for
    /// an output window, or [`crate::TARGET_FORMAT`] for an offscreen target.
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("blit"),
                source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
            });

        let uniform_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("blit uniform"),
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

        let source_layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blit source"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                ],
            });

        let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit"),
            size: std::mem::size_of::<BlitUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });

        // Nearest, and non-filtering in the bind group layout to match. One
        // canvas pixel is one LED; see the note at the top of blit.wgsl.
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(&uniform_layout), Some(&source_layout)],
                immediate_size: 0,
            });

        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });

        Self {
            pipeline,
            uniform,
            uniform_bind_group,
            source_layout,
            sampler,
            format,
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// A bind group for a texture this pass can draw from.
    pub fn source(&self, gpu: &Gpu, view: &wgpu::TextureView) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit source"),
            layout: &self.source_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Draw `region` of `source` (whose full size is `source_size`) across the
    /// whole of `target`.
    ///
    /// A region reaching outside the source is clamped by the sampler rather than
    /// wrapping, so an output configured past the edge of the canvas shows a
    /// smear of the edge pixels — visibly wrong, which is the point.
    pub fn draw(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        source: &wgpu::BindGroup,
        source_size: Size,
        region: Rect,
    ) {
        let uv = Quad::from_rect(region).to_uv(source_size.as_vec2());
        gpu.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&BlitUniform {
                src_rect: [
                    uv.tl.x,
                    uv.tl.y,
                    (uv.br.x - uv.tl.x).max(f32::EPSILON),
                    (uv.br.y - uv.tl.y).max(f32::EPSILON),
                ],
            }),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // The triangle covers the whole target, so nothing needs
                    // clearing first.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.set_bind_group(1, source, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_becomes_normalised_texture_coordinates() {
        // The maths the draw call performs, checked without a GPU.
        let uv = Quad::from_rect(Rect::new(960.0, 0.0, 960.0, 540.0))
            .to_uv(Size::new(1920, 1080).as_vec2());
        assert_eq!(uv.tl, unmapper_core::Vec2::new(0.5, 0.0));
        assert_eq!(uv.br, unmapper_core::Vec2::new(1.0, 0.5));
    }
}
