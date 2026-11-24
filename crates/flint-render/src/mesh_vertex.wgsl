// A mesh's vertex products, written once (#1244): the transform every mesh
// draw binds at group 0, and where a vertex lands in the world and on the
// screen. Every pass that draws a mesh's triangles again and tests them
// against the scene's depth (the style mask, the selection mask) reaches
// the same depth only by computing the same products in the same order, so
// they all call these rather than repeat them. Before #1244 the struct was
// declared in seven shaders and the products written out in four, and a
// drift in one would have shown as holes in a mask, not as an error.
//
// `wgsl_sources.rs` puts this file in front of the shaders that take it.
// A woozy module (#215, #737) is built with the `//FLINT_WOOZE ` prefix cut
// from every line (`wgsl_sources::woozy`), which turns on the drift field
// and `drifted`; it needs wooze_noise.wgsl in front.

// One draw's slot (`pipeline::TransformUniforms`). A woozy draw's drift
// rides in the slot's last 48 bytes, past where anything else looks.
struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
    //FLINT_WOOZE wooze: ShallowWooze,
};

@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// A point in the model's space (skinned first, for a skinned mesh) in the
// world's
fn world_of(local: vec4<f32>) -> vec4<f32> {
    return transform.model * local;
}

// A point in the world's space on the screen
fn clip_of(world: vec4<f32>) -> vec4<f32> {
    return transform.view_proj * world;
}

// A normal in the model's space (skinned first) in the world's, unit length
fn world_normal_of(local: vec3<f32>) -> vec3<f32> {
    return normalize((transform.model_inv_transpose * vec4<f32>(local, 0.0)).xyz);
}

// Where a woozy draw puts a vertex whose rest is `rest` (`world_of`'s):
// moved by the drift, with w 1
//FLINT_WOOZE fn drifted(rest: vec4<f32>) -> vec4<f32> {
//FLINT_WOOZE     return vec4<f32>(rest.xyz + wooze_offset(rest.xyz, transform.wooze), 1.0);
//FLINT_WOOZE }
