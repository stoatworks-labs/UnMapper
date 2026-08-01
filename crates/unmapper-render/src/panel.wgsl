// Painting one panel's slice of a source onto the stage.
//
// Two entry points share one fragment shader, because the *sampling* problem is
// identical in both views and only the transform differs:
//
//   vs_canvas — emulation. Positions are virtual-raster pixels, mapped straight
//               to NDC. One canvas pixel is one LED.
//   vs_previz — 3D. Positions are metres in stage space, through a view-projection.
//
// # The uvq coordinate
//
// `uvq` is NOT a texture coordinate. It is `(u*q, v*q, q)`, and the fragment
// shader divides to recover `(u, v)`. That divide is what makes a corner-pinned
// slice sample correctly: interpolating u and v directly across two triangles
// gives a bilinear map with a visible kink along the shared diagonal, where the
// true map is projective. See `Quad::projective_weights` in unmapper-core for
// where q comes from and why an unwarped rect is unaffected.
//
// In vs_previz this composes with the GPU's own perspective-correct varying
// interpolation rather than fighting it: the hardware corrects for the 3D
// projection, and the divide below corrects for the source warp.

struct Globals {
    // Emulation: canvas size in pixels. Previz: unused.
    canvas_size: vec2<f32>,
    _pad: vec2<f32>,
    // Previz: view * projection. Emulation: unused.
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

@group(1) @binding(0) var src_texture: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;

struct VertexIn {
    // Canvas pixels (vs_canvas) or stage metres (vs_previz). The z is ignored by
    // vs_canvas, so one vertex buffer layout serves both.
    @location(0) position: vec3<f32>,
    @location(1) uvq: vec3<f32>,
    @location(2) tint: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uvq: vec3<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_canvas(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    // Pixels, origin top-left → NDC, origin centre and y up.
    let ndc = vec2<f32>(
        in.position.x / globals.canvas_size.x * 2.0 - 1.0,
        1.0 - in.position.y / globals.canvas_size.y * 2.0,
    );
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uvq = in.uvq;
    out.tint = in.tint;
    return out;
}

@vertex
fn vs_previz(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.uvq = in.uvq;
    out.tint = in.tint;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    // The projective divide. q is never zero for a quad that passed
    // `projective_weights` — a degenerate one returns all-ones there — but guard
    // anyway, because a NaN here would propagate across the whole panel.
    let q = max(in.uvq.z, 1e-6);
    let uv = in.uvq.xy / q;
    return textureSample(src_texture, src_sampler, uv) * in.tint;
}

// A flat colour, for panels with no source bound and for the canvas backdrop
// outline. Keeps a "nothing here" panel visible rather than invisible.
@fragment
fn fs_solid(in: VertexOut) -> @location(0) vec4<f32> {
    return in.tint;
}
