// Copying a sub-rectangle of the canvas onto a whole target.
//
// This is how one monitor becomes a piece of the wall: the emulation canvas is
// rendered once at full resolution, and each output blits the region it stands
// in for.
//
// # Nearest, not linear
//
// The sampler is deliberately NEAREST (see `blit.rs`). One canvas pixel is one
// LED, so interpolating between them is meaningless — and when the region and
// the monitor are not the same size, a linear filter would quietly hide that by
// producing a plausible blurry image. Nearest makes the mismatch visible as
// blockiness, which is the honest signal that the output is misconfigured.

struct Blit {
    // Source rectangle in 0..1 texture coordinates: xy = origin, zw = size.
    src_rect: vec4<f32>,
};

@group(0) @binding(0) var<uniform> blit: Blit;
@group(1) @binding(0) var src_texture: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VertexOut {
    // A single oversized triangle rather than two triangles for a quad: no
    // vertex buffer, no index buffer, and no seam down the diagonal.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let pos = positions[index];

    var out: VertexOut;
    out.clip_position = vec4<f32>(pos, 0.0, 1.0);
    // Clip space has y up, textures have y down.
    let local = vec2<f32>(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
    out.uv = blit.src_rect.xy + local * blit.src_rect.zw;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(src_texture, src_sampler, in.uv);
}
