// Inverted-hull outline shader for Blender-style selection highlighting.
// Renders back-faces pushed outward along normals in clip space for
// screen-space uniform line width. Outputs solid orange.
//
// Also provides depth-prepass entry points (no push) used in wireframe mode
// to mask the outline interior with front-face depth.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Bind group 3: Bone matrices (storage buffer, used by skinned variant only)
@group(3) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

// Outline thickness in NDC units (tuned for ~3px at 1080p)
const OUTLINE_WIDTH: f32 = 0.012;

// Blender selection orange
const OUTLINE_COLOR: vec4<f32> = vec4<f32>(1.0, 0.55, 0.0, 1.0);

// Weight for center-to-vertex expansion vs normal push.
// Higher values fill corners better on hard-edged geometry.
const CENTER_EXPAND_WEIGHT: f32 = 0.5;

// ===== Vertex inputs =====

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

fn skin_transform(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    return bone_matrices[joints.x] * weights.x
         + bone_matrices[joints.y] * weights.y
         + bone_matrices[joints.z] * weights.z
         + bone_matrices[joints.w] * weights.w;
}

/// Compute the outline push direction: blend normal direction with
/// center-to-vertex expansion to fill corners on hard-edged geometry.
fn outline_push(clip_pos: vec4<f32>, world_normal: vec3<f32>) -> vec2<f32> {
    // Normal direction in clip space
    let clip_normal = transform.view_proj * vec4<f32>(world_normal, 0.0);
    let ndc_normal = normalize(clip_normal.xy);

    // Center-to-vertex direction in NDC (fills corners on hard-edged geometry)
    let obj_center = vec4<f32>(transform.model[3].xyz, 1.0);
    let clip_center = transform.view_proj * obj_center;
    let center_to_vertex = clip_pos.xy / clip_pos.w - clip_center.xy / clip_center.w;
    let ctv_len = length(center_to_vertex);
    let expand_dir = select(
        vec2<f32>(0.0, 0.0),
        center_to_vertex / ctv_len,
        ctv_len > 0.0001
    );

    // Blend: normal push for smooth surfaces + center expansion for corners
    return normalize(ndc_normal + expand_dir * CENTER_EXPAND_WEIGHT);
}

// ===== Depth prepass (no push, front-face rendering) =====
// Used in wireframe mode to write the entity's front-face depth so
// the outline interior is masked by the depth buffer.

@vertex
fn vs_depth_prepass(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    out.clip_position = transform.view_proj * world_pos;
    return out;
}

@vertex
fn vs_skinned_depth_prepass(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;
    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let world_pos = transform.model * skinned_pos;
    out.clip_position = transform.view_proj * world_pos;
    return out;
}

// ===== Standard mesh outline =====

@vertex
fn vs_outline(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    let clip_pos = transform.view_proj * world_pos;

    let world_normal = normalize((transform.model_inv_transpose * vec4<f32>(in.normal, 0.0)).xyz);
    let push_dir = outline_push(clip_pos, world_normal);

    var pushed = clip_pos;
    pushed.x += push_dir.x * OUTLINE_WIDTH * clip_pos.w;
    pushed.y += push_dir.y * OUTLINE_WIDTH * clip_pos.w;

    out.clip_position = pushed;
    return out;
}

// ===== Skinned mesh outline =====

@vertex
fn vs_skinned_outline(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;

    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let skinned_normal = normalize((skin_mat * vec4<f32>(in.normal, 0.0)).xyz);

    let world_pos = transform.model * skinned_pos;
    let clip_pos = transform.view_proj * world_pos;

    let world_normal = normalize((transform.model_inv_transpose * vec4<f32>(skinned_normal, 0.0)).xyz);
    let push_dir = outline_push(clip_pos, world_normal);

    var pushed = clip_pos;
    pushed.x += push_dir.x * OUTLINE_WIDTH * clip_pos.w;
    pushed.y += push_dir.y * OUTLINE_WIDTH * clip_pos.w;

    out.clip_position = pushed;
    return out;
}

// ===== Shared fragment shader =====

@fragment
fn fs_outline() -> @location(0) vec4<f32> {
    return OUTLINE_COLOR;
}
