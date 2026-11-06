// ASCII spike (#261): the per-entity mask pass.
//
// Redraws the chosen entities' opaque draws after the scene pass, into an
// Rgba32Float side target: r = the entity's style id (0 = not stylized),
// gb = its anchor in scene pixels (the object grid's origin), a = 1.
//
// It has no depth attachment. Each fragment tests itself against the
// scene's single-sample depth (the resolved one under MSAA) with a
// tolerance of its own depth slope, so the mask needs nothing from the
// scene pass's pipelines and works at any render scale.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct MaskDraw {
    id: f32,
    // How far behind the scene's depth a fragment may be, in depth slopes
    slope_tolerance: f32,
    anchor: vec2<f32>,
};

@group(0) @binding(0) var<uniform> transform: TransformUniforms;
@group(1) @binding(0) var<uniform> draw: MaskDraw;
@group(1) @binding(1) var scene_depth: texture_depth_2d;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    // The same two products in the same order as shader.wgsl's vs_main, so
    // the depth matches the scene's wherever the rasterisers agree.
    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    return transform.view_proj * world_pos;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let slope = fwidth(frag.z);
    let scene = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    if frag.z > scene + slope * draw.slope_tolerance + 1e-6 {
        discard;
    }
    return vec4<f32>(draw.id, draw.anchor, 1.0);
}
