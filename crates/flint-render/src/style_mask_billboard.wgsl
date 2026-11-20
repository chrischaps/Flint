// The style mask's billboard draw (#1011), appended to the billboard module
// only (style_mask.rs, `billboard_mask_source`): tear_mask.wgsl, then
// style_mask.wgsl without its mesh blocks (the transform, the bones and
// the mesh vertex stages, whose bindings these take), then this file.
//
// The quad is placed exactly as billboard_shader.wgsl places it, with the
// same products in the same order, so its depths are the ones the scene
// wrote, and cut out by the same alpha test at the same uv, so only the
// sprite's own shape takes its style. The rest is fs_main's and fs_msaa's:
// the depth test against the scene, the veil, the written blend.

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
// Group 1 is the mask's draw group, as for a mesh
@group(2) @binding(0) var sprite_texture: texture_2d<f32>;
@group(2) @binding(1) var sprite_sampler: sampler;

struct BillboardOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_billboard(@builtin(vertex_index) vertex_index: u32) -> BillboardOut {
    // Corners BL, BR, TL, TR, as billboard_shader.wgsl's switch gives them
    // (no switch here: #314's DX12/FXC trap); every value is exact
    let corner = vec2<f32>(f32(vertex_index & 1u), f32((vertex_index >> 1u) & 1u));
    let local_x = corner.x - 0.5;
    let local_y = corner.y;
    let u = corner.x;
    let v = 1.0 - corner.y;

    let fx = f32(sprite.frames_x);
    let fy = f32(sprite.frames_y);
    let frame_col = f32(sprite.frame % sprite.frames_x);
    let frame_row = f32(sprite.frame / sprite.frames_x);
    let cell_w = 1.0 / fx;
    let cell_h = 1.0 / fy;
    let final_u = (frame_col + u) * cell_w;
    let final_v = (frame_row + v) * cell_h;

    let adjusted_y = local_y - sprite.anchor_y;

    let world = sprite.world_pos
        + billboard.camera_right * local_x * sprite.width
        + billboard.camera_up * adjusted_y * sprite.height;

    var out: BillboardOut;
    out.clip_position = billboard.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    return out;
}

// The sprite's alpha, sampled as fs_billboard samples it. Called first, in
// uniform control flow, as its derivatives need.
fn sprite_alpha(uv: vec2<f32>) -> f32 {
    return textureSample(sprite_texture, sprite_sampler, uv).a;
}

@fragment
fn fs_billboard(in: BillboardOut) -> @location(0) vec2<u32> {
    let frag = in.clip_position;
    let slope = fwidth(frag.z);
    // Cut out where the scene's sprite is (binary alpha, at 0.5)
    if sprite_alpha(in.uv) < 0.5 {
        discard;
    }
    let scene = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    if frag.z > scene + slope * draw.slope_tolerance + 1e-6 {
        discard;
    }
    let covered = clamp(textureLoad(veil, vec2<i32>(frag.xy), 0).r, 0.0, 1.0);
    return vec2<u32>(draw.id, written_blend(frag.xy, covered));
}

// Under MSAA (#306), as fs_msaa: the main pass's alpha test discards a
// whole pixel or none of it, so the samples kept are the triangle's
// coverage that passes the scene's depth.
@fragment
fn fs_billboard_msaa(
    in: BillboardOut,
    @builtin(sample_mask) covered: u32,
) -> MsaaOut {
    let frag = in.clip_position;
    let slope = fwidth(frag.z);
    if sprite_alpha(in.uv) < 0.5 {
        discard;
    }
    let px = vec2<i32>(frag.xy);
    let limit = frag.z - slope * draw.slope_tolerance - 1e-6;
    var keep = 0u;
    for (var i = 0u; i < 4u; i++) {
        let bit = 1u << i;
        if (covered & bit) != 0u && textureLoad(scene_depth_ms, px, i32(i)) >= limit {
            keep |= bit;
        }
    }
    if keep == 0u {
        discard;
    }
    let veiled = clamp(textureLoad(veil, px, 0).r, 0.0, 1.0);
    var out: MsaaOut;
    out.value = vec2<u32>(draw.id, written_blend(frag.xy, veiled));
    out.samples = keep;
    return out;
}
