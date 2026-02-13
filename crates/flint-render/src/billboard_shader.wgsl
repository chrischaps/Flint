// Billboard sprite shader — camera-facing quads with sprite sheet support
// Binary alpha (discard < 0.5) for Doom-authentic transparency

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

@group(0) @binding(0)
var<uniform> billboard: BillboardUniforms;

@group(0) @binding(1)
var<uniform> sprite: SpriteInstance;

@group(1) @binding(0)
var sprite_texture: texture_2d<f32>;
@group(1) @binding(1)
var sprite_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Unit quad: 4 vertices, indexed as triangle list
// Vertex index 0..3 maps to corners of a [-0.5, 0.5] quad
@vertex
fn vs_billboard(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Quad corners: BL, BR, TL, TR
    var local_x: f32;
    var local_y: f32;
    var u: f32;
    var v: f32;

    switch vertex_index {
        case 0u: { // Bottom-left
            local_x = -0.5;
            local_y = 0.0;
            u = 0.0;
            v = 1.0;
        }
        case 1u: { // Bottom-right
            local_x = 0.5;
            local_y = 0.0;
            u = 1.0;
            v = 1.0;
        }
        case 2u: { // Top-left
            local_x = -0.5;
            local_y = 1.0;
            u = 0.0;
            v = 0.0;
        }
        case 3u: { // Top-right
            local_x = 0.5;
            local_y = 1.0;
            u = 1.0;
            v = 0.0;
        }
        default: {
            local_x = 0.0;
            local_y = 0.0;
            u = 0.0;
            v = 0.0;
        }
    }

    // Sprite sheet UV sub-rectangle
    let fx = f32(sprite.frames_x);
    let fy = f32(sprite.frames_y);
    let frame_col = f32(sprite.frame % sprite.frames_x);
    let frame_row = f32(sprite.frame / sprite.frames_x);
    let cell_w = 1.0 / fx;
    let cell_h = 1.0 / fy;
    let final_u = (frame_col + u) * cell_w;
    let final_v = (frame_row + v) * cell_h;

    // Offset local_y by anchor (0 = bottom-anchored, 0.5 = center-anchored)
    let adjusted_y = local_y - sprite.anchor_y;

    // Billboard in world space
    let world = sprite.world_pos
        + billboard.camera_right * local_x * sprite.width
        + billboard.camera_up * adjusted_y * sprite.height;

    var out: VertexOutput;
    out.clip_position = billboard.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    return out;
}

@fragment
fn fs_billboard(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(sprite_texture, sprite_sampler, in.uv);

    // Binary alpha — Doom-authentic hard cutout
    if color.a < 0.5 {
        discard;
    }

    return vec4<f32>(color.rgb, 1.0);
}

// ===== Outline entry points for billboard sprites =====
// Renders a slightly larger quad with solid orange, alpha-tested.
// The normal-size sprite covers the interior, leaving only the outline visible.

const OUTLINE_MARGIN: f32 = 0.08; // extra fraction of width/height for outline
const OUTLINE_COLOR: vec4<f32> = vec4<f32>(1.0, 0.55, 0.0, 1.0);

@vertex
fn vs_billboard_outline(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var local_x: f32;
    var local_y: f32;
    var u: f32;
    var v: f32;

    switch vertex_index {
        case 0u: {
            local_x = -0.5;
            local_y = 0.0;
            u = 0.0;
            v = 1.0;
        }
        case 1u: {
            local_x = 0.5;
            local_y = 0.0;
            u = 1.0;
            v = 1.0;
        }
        case 2u: {
            local_x = -0.5;
            local_y = 1.0;
            u = 0.0;
            v = 0.0;
        }
        case 3u: {
            local_x = 0.5;
            local_y = 1.0;
            u = 1.0;
            v = 0.0;
        }
        default: {
            local_x = 0.0;
            local_y = 0.0;
            u = 0.0;
            v = 0.0;
        }
    }

    let fx = f32(sprite.frames_x);
    let fy = f32(sprite.frames_y);
    let frame_col = f32(sprite.frame % sprite.frames_x);
    let frame_row = f32(sprite.frame / sprite.frames_x);
    let cell_w = 1.0 / fx;
    let cell_h = 1.0 / fy;
    let final_u = (frame_col + u) * cell_w;
    let final_v = (frame_row + v) * cell_h;

    let adjusted_y = local_y - sprite.anchor_y;

    // Scale up the quad by the outline margin
    let outline_w = sprite.width * (1.0 + OUTLINE_MARGIN);
    let outline_h = sprite.height * (1.0 + OUTLINE_MARGIN);

    let world = sprite.world_pos
        + billboard.camera_right * local_x * outline_w
        + billboard.camera_up * adjusted_y * outline_h;

    var out: VertexOutput;
    out.clip_position = billboard.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    return out;
}

@fragment
fn fs_billboard_outline(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(sprite_texture, sprite_sampler, in.uv);

    // Only draw outline where the sprite has alpha (avoid orange rectangle)
    if color.a < 0.5 {
        discard;
    }

    return OUTLINE_COLOR;
}
