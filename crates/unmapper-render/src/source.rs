//! GPU textures for incoming frames.

use std::collections::HashMap;

use unmapper_core::Size;

use crate::gpu::Gpu;

/// One source's texture on the GPU.
pub struct SourceTexture {
    texture: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
    pub size: Size,
    format: wgpu::TextureFormat,
    /// The sequence number of the frame currently uploaded, so the same frame is
    /// never uploaded twice.
    pub sequence: u64,
}

impl SourceTexture {
    fn create(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        size: Size,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
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
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Self {
            texture,
            bind_group,
            size,
            format,
            sequence: 0,
        }
    }

    fn upload(&mut self, gpu: &Gpu, data: &[u8], stride: usize) {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // Senders pad rows, so the stride from the frame is used rather
                // than assuming `width * 4`. Getting this wrong shears the image
                // diagonally — a distinctive and easily misread symptom.
                bytes_per_row: Some(stride as u32),
                rows_per_image: Some(self.size.height.max(1)),
            },
            wgpu::Extent3d {
                width: self.size.width.max(1),
                height: self.size.height.max(1),
                depth_or_array_layers: 1,
            },
        );
    }
}

/// One frame's worth of pixels on their way to the GPU.
///
/// A struct rather than six positional parameters, because `bgra` and the two
/// `u32` dimensions are exactly the kind of arguments that get transposed at a
/// call site without the compiler noticing.
pub struct FrameUpload<'a> {
    pub width: u32,
    pub height: u32,
    /// Bytes per row, which is **not** always `width * 4` — senders pad.
    pub stride: usize,
    /// Whether `data` is BGRA rather than RGBA.
    pub bgra: bool,
    pub data: &'a [u8],
    pub sequence: u64,
}

/// Every source's texture, plus the placeholder used when nothing is bound.
pub struct SourceTextures {
    pub layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<String, SourceTexture>,
    /// A 1x1 opaque texture, so a panel with no frame yet still draws its tint
    /// rather than failing to bind.
    pub placeholder: SourceTexture,
}

impl SourceTextures {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("source"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("source"),
            // Clamp, because a slice sitting exactly on the edge of its raster
            // would otherwise wrap and pull a stripe of the opposite edge onto
            // the wall.
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let mut placeholder = SourceTexture::create(
            gpu,
            &layout,
            &sampler,
            Size::new(1, 1),
            wgpu::TextureFormat::Rgba8Unorm,
            "placeholder",
        );
        placeholder.upload(gpu, &[255, 255, 255, 255], 4);

        Self {
            layout,
            sampler,
            textures: HashMap::new(),
            placeholder,
        }
    }

    pub fn get(&self, source_id: &str) -> Option<&SourceTexture> {
        self.textures.get(source_id)
    }

    /// The size to use when turning a pixel-space quad into texture coordinates.
    ///
    /// The live frame's own size wins over whatever the slice map expected,
    /// because the sender is the authority on what it is actually sending — a
    /// Resolume output reconfigured to 4K mid-show must not keep being sampled as
    /// though it were 1080p.
    pub fn source_size(&self, source_id: &str, expected: Option<Size>) -> Option<Size> {
        self.textures.get(source_id).map(|t| t.size).or(expected)
    }

    /// Upload a frame, creating or resizing the texture as needed.
    pub fn upload(&mut self, gpu: &Gpu, source_id: &str, frame: FrameUpload<'_>) {
        let FrameUpload {
            width,
            height,
            stride,
            bgra,
            data,
            sequence,
        } = frame;
        if width == 0 || height == 0 {
            return;
        }
        // Picking the texture format to match the wire format means a BGRA sender
        // costs nothing — the GPU swizzles on read instead of the CPU rewriting
        // every pixel of every frame.
        let format = if bgra {
            wgpu::TextureFormat::Bgra8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let size = Size::new(width, height);

        let stale = self
            .textures
            .get(source_id)
            .map(|t| t.size != size || t.format != format)
            .unwrap_or(true);

        if stale {
            let tex =
                SourceTexture::create(gpu, &self.layout, &self.sampler, size, format, source_id);
            self.textures.insert(source_id.to_owned(), tex);
        }

        if let Some(tex) = self.textures.get_mut(source_id) {
            tex.upload(gpu, data, stride);
            tex.sequence = sequence;
        }
    }

    /// Drop textures for sources that no longer exist.
    pub fn retain(&mut self, live: &[String]) {
        self.textures.retain(|id, _| live.contains(id));
    }
}
