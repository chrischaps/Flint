// Shallow wooze (#215): the vertex stage of a woozy draw, standard and skinned.
//
// These are vs_main and vs_skinned with one line added. They pair with the
// fragment stages of shader.wgsl and skinned_shader.wgsl, which stay as they are.
// The transform here is the same uniform those read, seen through a longer struct:
// the drift lives in the slot's last 48 bytes, past where anything else looks.

struct WoozyTransform {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
    wooze: ShallowWooze,
};

@group(0) @binding(0)
var<uniform> transform: WoozyTransform;

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

    let rest = transform.model * vec4<f32>(in.position, 1.0);
    let world_pos = rest.xyz + wooze_offset(rest.xyz, transform.wooze);
    out.clip_position = transform.view_proj * vec4<f32>(world_pos, 1.0);
    out.color = in.color;
    out.normal = normalize((transform.model_inv_transpose * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_pos = world_pos;
    out.uv = in.uv;

    return out;
}

@vertex
fn vs_skinned_wooze(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;

    let skin_mat = bone_matrices[in.joint_indices.x] * in.joint_weights.x
                 + bone_matrices[in.joint_indices.y] * in.joint_weights.y
                 + bone_matrices[in.joint_indices.z] * in.joint_weights.z
                 + bone_matrices[in.joint_indices.w] * in.joint_weights.w;
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let skinned_normal = normalize((skin_mat * vec4<f32>(in.normal, 0.0)).xyz);

    let rest = transform.model * skinned_pos;
    let world_pos = rest.xyz + wooze_offset(rest.xyz, transform.wooze);
    out.clip_position = transform.view_proj * vec4<f32>(world_pos, 1.0);
    out.color = in.color;
    out.normal = normalize((transform.model_inv_transpose * vec4<f32>(skinned_normal, 0.0)).xyz);
    out.world_pos = world_pos;
    out.uv = in.uv;

    return out;
}
