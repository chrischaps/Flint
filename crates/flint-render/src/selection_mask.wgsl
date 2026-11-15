// The editor's selection mask (#602): every draw of the selected entity and
// its descendants, drawn again after the scene pass into an Rg8Unorm target
// at scene resolution.
//
//   .r = 1 where the selection covers the pixel, whatever is in front of it
//   .g = 1 where it is also the nearest surface (it passes the scene's depth)
//
// No depth attachment: each fragment tests itself against the frame's
// single-sample depth with a tolerance of its own depth slope, as the style
// mask does, and the target blends with max, so overlapping draws keep the
// most of each channel. The vertex stages repeat the scene shaders' products
// in the same order so the depths agree.
//
// selection_outline.wgsl turns the mask into the outline over the finished
// image. This file is prefixed with wooze_noise.wgsl for the woozy module
// only; FLINT_WOOZE marks the lines that differ.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
    //FLINT_WOOZE wooze: ShallowWooze,
};

@group(0) @binding(0) var<uniform> transform: TransformUniforms;
@group(1) @binding(0) var scene_depth: texture_depth_2d;
// Skinned draws only; the static layout leaves group 2 out
@group(2) @binding(0) var<storage, read> bone_matrices: array<mat4x4<f32>>;

// How far behind the scene's depth a fragment may be: two depth slopes, but
// never more than MAX_TOLERANCE. The redraw repeats the scene's vertex maths,
// so a visible fragment lands on the scene's depth to a rounding error; the
// slope covers a render scale or an MSAA resolve. Uncapped, a surface seen
// edge-on (a silhouette, its slope huge) passed from behind the floor and
// dotted the hidden outline with visible ones.
const SLOPE_TOLERANCE: f32 = 2.0;
const MAX_TOLERANCE: f32 = 4e-6;

struct StaticVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct SkinnedVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) joint_indices: vec4<u32>,
    @location(5) joint_weights: vec4<f32>,
};

fn to_clip(rest: vec4<f32>) -> vec4<f32> {
    var world = rest.xyz;
    //FLINT_WOOZE world = world + wooze_offset(world, transform.wooze);
    return transform.view_proj * vec4<f32>(world, 1.0);
}

@vertex
fn vs_static(in: StaticVertex) -> @builtin(position) vec4<f32> {
    return to_clip(transform.model * vec4<f32>(in.position, 1.0));
}

@vertex
fn vs_skinned(in: SkinnedVertex) -> @builtin(position) vec4<f32> {
    let j = in.joint_indices;
    let w = in.joint_weights;
    let skin = bone_matrices[j.x] * w.x
             + bone_matrices[j.y] * w.y
             + bone_matrices[j.z] * w.z
             + bone_matrices[j.w] * w.w;
    return to_clip(transform.model * (skin * vec4<f32>(in.position, 1.0)));
}

fn mask_value(frag: vec4<f32>) -> vec4<f32> {
    let slope = fwidth(frag.z);
    let scene = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    let visible = frag.z <= scene + min(slope * SLOPE_TOLERANCE, MAX_TOLERANCE) + 1e-6;
    return vec4<f32>(1.0, select(0.0, 1.0, visible), 0.0, 0.0);
}

@fragment
fn fs_mask(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return mask_value(frag);
}
