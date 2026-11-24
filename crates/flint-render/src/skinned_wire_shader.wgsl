// The skinned wireframe's stages: each vertex posed by its bones, drawn as
// flat black edge lines. What was left of the old inverted-hull outline
// shader when the selection outline moved over the finished image (#602).
// mesh_vertex.wgsl and skinning.wgsl go in front (`wgsl_sources::WIRE`,
// #1244), so the lines lie on the triangles the scene drew.

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
    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    return clip_of(world_of(skin_mat * vec4<f32>(in.position, 1.0)));
}

// Flat black
@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
