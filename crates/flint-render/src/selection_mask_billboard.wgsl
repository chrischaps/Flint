// The selection mask's billboard draw (#602): the sprite's quad exactly as
// billboard_shader.wgsl places it, cut out by the same alpha test, so the
// outline follows the sprite's shape rather than its rectangle. Writes the
// mask value selection_mask.wgsl describes.

struct BillboardUniforms {
    view_proj: mat4x4<f32>,
    camera_right: vec3<f32>,
    _pad0: f32,
    camera_up: vec3<f32>,
    _pad1: f32,
};

struct SpriteInstance {
    world_pos: vec3<f32>,
    width: f32,
    height: f32,
    frame: u32,
    frames_x: u32,
    frames_y: u32,
    anchor_y: f32,
    fullbright: u32,
    selection_highlight: u32,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> billboard: BillboardUniforms;
@group(0) @binding(1) var<uniform> sprite: SpriteInstance;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;
@group(2) @binding(0) var scene_depth: texture_depth_2d;

// As selection_mask.wgsl's
const SLOPE_TOLERANCE: f32 = 2.0;
const MAX_TOLERANCE: f32 = 4e-6;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_billboard(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Corners BL, BR, TL, TR, as billboard_shader.wgsl's (no switch: #314's
    // DX12/FXC trap)
    let corner = vec2<f32>(f32(vertex_index & 1u), f32((vertex_index >> 1u) & 1u));
    let local_x = corner.x - 0.5;
    let local_y = corner.y;
    let u = corner.x;
    let v = 1.0 - corner.y;

    let frame_col = f32(sprite.frame % sprite.frames_x);
    let frame_row = f32(sprite.frame / sprite.frames_x);
    let final_u = (frame_col + u) / f32(sprite.frames_x);
    let final_v = (frame_row + v) / f32(sprite.frames_y);
    let adjusted_y = local_y - sprite.anchor_y;

    let world = sprite.world_pos
        + billboard.camera_right * local_x * sprite.width
        + billboard.camera_up * adjusted_y * sprite.height;

    var out: VertexOutput;
    out.clip_position = billboard.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    return out;
}

@fragment
fn fs_mask(in: VertexOutput) -> @location(0) vec4<f32> {
    let slope = fwidth(in.clip_position.z);
    let alpha = textureSample(sprite_texture, sprite_sampler, in.uv).a;
    if alpha < 0.5 {
        discard;
    }
    let scene = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
    let visible = in.clip_position.z <= scene + min(slope * SLOPE_TOLERANCE, MAX_TOLERANCE) + 1e-6;
    return vec4<f32>(1.0, select(0.0, 1.0, visible), 0.0, 0.0);
}
