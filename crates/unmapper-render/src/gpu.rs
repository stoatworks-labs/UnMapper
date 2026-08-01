//! Device setup and the shared render resources.

use anyhow::{anyhow, Result};

/// The canvas and every offscreen target use this. **Not** an sRGB format: the
/// pixels being moved are already whatever Resolume sent, and re-encoding them
/// would shift every colour on the wall. A value of 128 in must be 128 out.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A wgpu device and queue, plus the pipelines built on them.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub backend: wgpu::Backend,
}

impl Gpu {
    /// Create a headless device — no window, no surface.
    ///
    /// The app creates its surfaces afterwards against this same device, which is
    /// what lets one device serve every output window plus the offscreen targets
    /// that feed NDI, Syphon and Spout.
    pub async fn new() -> Result<Self> {
        Self::with_compatible_surface(None).await
    }

    pub async fn with_compatible_surface(surface: Option<&wgpu::Surface<'_>>) -> Result<Self> {
        let instance = wgpu::Instance::default();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow!("no suitable GPU adapter: {e}"))?;

        let info = adapter.get_info();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("unmapper"),
                required_features: wgpu::Features::empty(),
                // Default limits keep the Windows/Linux and low-end paths open.
                // A big rig needs large textures, so raise only that.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..wgpu::Limits::default()
                },
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| anyhow!("could not open the GPU device: {e}"))?;

        Ok(Self {
            device,
            queue,
            adapter_name: info.name,
            backend: info.backend,
        })
    }

    /// Block on [`Gpu::new`], for callers that are not already async.
    pub fn new_blocking() -> Result<Self> {
        pollster::block_on(Self::new())
    }
}

/// What the vertex buffers carry. One layout serves both the canvas and previz
/// pipelines; the canvas simply ignores `position.z`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// `(u*q, v*q, q)` — see `panel.wgsl`.
    pub uvq: [f32; 3],
    pub tint: [f32; 4],
}

impl Vertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
    };
}

/// The uniform block shared by both pipelines. Must match `Globals` in the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub canvas_size: [f32; 2],
    pub _pad: [f32; 2],
    pub view_proj: [[f32; 4]; 4],
}

impl Default for Globals {
    fn default() -> Self {
        Self {
            canvas_size: [1920.0, 1080.0],
            _pad: [0.0; 2],
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_block_is_std140_compatible() {
        // A mat4 must start on a 16-byte boundary. vec2 + vec2 padding is what
        // puts it there; drop the padding and every panel lands somewhere wrong.
        assert_eq!(std::mem::size_of::<Globals>(), 16 + 64);
        assert_eq!(std::mem::align_of::<Globals>(), 4);
        let g = Globals::default();
        let base = &g as *const _ as usize;
        assert_eq!(&g.view_proj as *const _ as usize - base, 16);
    }

    #[test]
    fn vertex_stride_matches_its_attributes() {
        assert_eq!(std::mem::size_of::<Vertex>(), (3 + 3 + 4) * 4);
        assert_eq!(Vertex::LAYOUT.array_stride, 40);
    }
}
