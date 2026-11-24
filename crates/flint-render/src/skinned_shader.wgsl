// Flint skinned mesh shader: GPU vertex skinning through bone matrices
// (skinning.wgsl's `skin_transform`), over the same surface as shader.wgsl
// (pbr_surface.wgsl, mesh_vertex.wgsl and the lighting files;
// `wgsl_sources::SKINNED`).

// Bind group 3: Bone matrices (storage buffer for arbitrary bone counts)
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
fn vs_skinned(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;

    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let skinned_normal = normalize((skin_mat * vec4<f32>(in.normal, 0.0)).xyz);

    let world_pos = world_of(skinned_pos);
    out.clip_position = clip_of(world_pos);
    out.color = in.color;
    out.normal = world_normal_of(skinned_normal);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}

@fragment
fn fs_skinned(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(facing(in, front));
}

// A living prop's echo that may only add light (`[living] echo_blend =
// "lighten"`): its colour premultiplied by its opacity, for a max blend, so a
// pixel ends as the brighter of what was there and the echo's share of light.
@fragment
fn fs_skinned_lighten(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let c = shade(facing(in, front));
    return vec4<f32>(c.rgb * c.a, c.a);
}
