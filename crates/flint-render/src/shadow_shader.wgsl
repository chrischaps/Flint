// Shadow map depth-only shader
// Renders geometry from light perspective for shadow mapping.
// Includes a skinned variant that applies bone matrices before world transform.

struct ShadowUniforms {
    light_view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> shadow: ShadowUniforms;

// Bind group 1: Bone matrices for skinned shadow pass
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

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_shadow(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = shadow.model * vec4<f32>(in.position, 1.0);
    out.clip_position = shadow.light_view_proj * world_pos;
    return out;
}

fn skin_transform(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    return bone_matrices[joints.x] * weights.x
         + bone_matrices[joints.y] * weights.y
         + bone_matrices[joints.z] * weights.z
         + bone_matrices[joints.w] * weights.w;
}

@vertex
fn vs_skinned_shadow(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;
    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let world_pos = shadow.model * skinned_pos;
    out.clip_position = shadow.light_view_proj * world_pos;
    return out;
}
