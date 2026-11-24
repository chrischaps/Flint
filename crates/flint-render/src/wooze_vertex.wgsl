// Shallow wooze (#215): the vertex stage of a woozy draw, standard and skinned.
//
// These are vs_main and vs_skinned with one call added (`drifted`). They pair
// with the fragment stages of shader.wgsl and skinned_shader.wgsl, which stay as
// they are. The transform is mesh_vertex.wgsl's with its drift field on
// (`wgsl_sources::wooze_vertex`): the same uniform those read, seen through a
// longer struct, the drift in the slot's last 48 bytes, past where anything else
// looks.

// Bind group 3: bone matrices, in the skinned variant only.
@group(3) @binding(0)
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
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_wooze(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let world_pos = drifted(world_of(vec4<f32>(in.position, 1.0)));
    out.clip_position = clip_of(world_pos);
    out.color = in.color;
    out.normal = world_normal_of(in.normal);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}

@vertex
fn vs_skinned_wooze(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;

    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let skinned_normal = normalize((skin_mat * vec4<f32>(in.normal, 0.0)).xyz);

    let world_pos = drifted(world_of(skinned_pos));
    out.clip_position = clip_of(world_pos);
    out.color = in.color;
    out.normal = world_normal_of(skinned_normal);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}
