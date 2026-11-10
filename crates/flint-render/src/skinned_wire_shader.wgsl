// The skinned wireframe's stages: each vertex posed by its bones, drawn as
// flat black edge lines. What was left of the old inverted-hull outline
// shader when the selection outline moved over the finished image (#602).

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Bind group 3: Bone matrices
@group(3) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) joint_indices: vec4<u32>,
    @location(5) joint_weights: vec4<f32>,
};

@vertex
fn vs_skinned_wire(in: SkinnedVertexInput) -> @builtin(position) vec4<f32> {
    let skin_mat = bone_matrices[in.joint_indices.x] * in.joint_weights.x
                 + bone_matrices[in.joint_indices.y] * in.joint_weights.y
                 + bone_matrices[in.joint_indices.z] * in.joint_weights.z
                 + bone_matrices[in.joint_indices.w] * in.joint_weights.w;
    let world_pos = transform.model * (skin_mat * vec4<f32>(in.position, 1.0));
    return transform.view_proj * world_pos;
}

// Flat black
@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
