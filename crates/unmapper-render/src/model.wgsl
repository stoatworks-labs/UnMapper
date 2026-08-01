// The set model, as context behind the panels in the previz view.
//
// Deliberately plainly shaded: this geometry exists so the operator can see
// where the walls sit relative to the set, not to look like a render. Anything
// glossier would compete with the video, which is the thing actually being
// judged.

struct ModelGlobals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    // Base colour, and how strongly to shade. A model with no normals gets
    // flat-shaded rather than black.
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: ModelGlobals;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
};

@vertex
fn vs(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = globals.model * vec4<f32>(in.position, 1.0);
    out.clip_position = globals.view_proj * world;
    // The model transform is rotation, uniform scale and translation only — no
    // shear — so the upper 3x3 rotates normals correctly without needing an
    // inverse transpose. `arrange_panels_from_layout` and the stage file both
    // guarantee that; a sheared model matrix would need the full treatment.
    out.world_normal = normalize((globals.model * vec4<f32>(in.normal, 0.0)).xyz);
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    // A key light over the operator's shoulder plus a floor bounce, so the far
    // side of a wall is dim rather than pure black.
    let key = normalize(vec3<f32>(0.35, 0.8, 0.5));
    let n = normalize(in.world_normal);
    let lambert = max(dot(n, key), 0.0);
    let bounce = max(dot(n, vec3<f32>(0.0, -1.0, 0.0)), 0.0) * 0.15;
    let shade = 0.25 + 0.75 * lambert + bounce;
    return vec4<f32>(globals.tint.rgb * shade, globals.tint.a);
}
