// Shallow wooze (#215): the shadow pass of a woozy draw. A caster moves by the same
// wooze_offset as it does in the main pass, on the same numbers, so its shadow stays
// attached. These are vs_shadow and vs_skinned_shadow with one line added.

struct WoozyShadowUniforms {
    light_view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    wooze: ShallowWooze,
};

@group(0) @binding(0)
var<uniform> shadow: WoozyShadowUniforms;

// Bind group 1: bone matrices, in the skinned variant only.
@group(1) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) joint_indices: vec4<u32>,
    @location(5) joint_weights: vec4<f32>,
};

@vertex
fn vs_shadow_wooze(in: VertexInput) -> @builtin(position) vec4<f32> {
    let rest = shadow.model * vec4<f32>(in.position, 1.0);
    let world_pos = rest.xyz + wooze_offset(rest.xyz, shadow.wooze);
    return shadow.light_view_proj * vec4<f32>(world_pos, 1.0);
}

@vertex
fn vs_skinned_shadow_wooze(in: SkinnedVertexInput) -> @builtin(position) vec4<f32> {
    let skin_mat = bone_matrices[in.joint_indices.x] * in.joint_weights.x
                 + bone_matrices[in.joint_indices.y] * in.joint_weights.y
                 + bone_matrices[in.joint_indices.z] * in.joint_weights.z
                 + bone_matrices[in.joint_indices.w] * in.joint_weights.w;
    let rest = shadow.model * (skin_mat * vec4<f32>(in.position, 1.0));
    let world_pos = rest.xyz + wooze_offset(rest.xyz, shadow.wooze);
    return shadow.light_view_proj * vec4<f32>(world_pos, 1.0);
}
