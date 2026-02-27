// Sprite2D shader — GPU-instanced flat XY-plane quads for 2D sprite rendering
// Reads instance data from a storage buffer. Sprites are axis-aligned on the XY plane
// (not camera-facing billboards). Layer ordering via Z-offset in depth buffer.

struct Sprite2dUniforms {
    view_proj: mat4x4<f32>,
};

struct Sprite2dInstance {
    pos_layer: vec4<f32>,    // xy = world position, z = anchor_x (0-1), w = layer * 0.01
    size: vec4<f32>,         // x = width, y = height, z = anchor_y (0-1), w = unused
    uv_rect: vec4<f32>,      // x = u_min, y = v_min, z = u_max, w = v_max
    tint: vec4<f32>,         // rgba color multiplier
    flags: vec4<f32>,        // x = flip_x (0/1), y = flip_y (0/1), z = unused, w = unused
};

@group(0) @binding(0)
var<uniform> uniforms: Sprite2dUniforms;

@group(1) @binding(0)
var<storage, read> instances: array<Sprite2dInstance>;

@group(2) @binding(0)
var sprite_texture: texture_2d<f32>;
@group(2) @binding(1)
var sprite_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_sprite2d(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let inst = instances[instance_index];
    let world_xy = inst.pos_layer.xy;
    let anchor_x = inst.pos_layer.z;
    let z_offset = inst.pos_layer.w;
    let width = inst.size.x;
    let height = inst.size.y;
    let anchor_y = inst.size.z;

    // Unit quad corners: BL(0), BR(1), TL(2), TR(3)
    // Offset by anchor so (0,0) is at the anchor point
    var local_x: f32;
    var local_y: f32;
    var u: f32;
    var v: f32;

    switch vertex_index {
        case 0u: {
            local_x = -anchor_x;
            local_y = -anchor_y;
            u = 0.0;
            v = 1.0;
        }
        case 1u: {
            local_x = 1.0 - anchor_x;
            local_y = -anchor_y;
            u = 1.0;
            v = 1.0;
        }
        case 2u: {
            local_x = -anchor_x;
            local_y = 1.0 - anchor_y;
            u = 0.0;
            v = 0.0;
        }
        case 3u: {
            local_x = 1.0 - anchor_x;
            local_y = 1.0 - anchor_y;
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

    // Apply flip
    let flip_x = inst.flags.x;
    let flip_y = inst.flags.y;
    if flip_x > 0.5 {
        u = 1.0 - u;
    }
    if flip_y > 0.5 {
        v = 1.0 - v;
    }

    // Map parametric UV [0,1] through the source_rect UV window
    let uv_min = inst.uv_rect.xy;
    let uv_max = inst.uv_rect.zw;
    let final_u = uv_min.x + u * (uv_max.x - uv_min.x);
    let final_v = uv_min.y + v * (uv_max.y - uv_min.y);

    // World position: flat on XY plane with Z-offset for layer ordering
    let world_pos = vec3<f32>(
        world_xy.x + local_x * width,
        world_xy.y + local_y * height,
        z_offset,
    );

    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = vec2<f32>(final_u, final_v);
    out.tint = inst.tint;
    return out;
}

@fragment
fn fs_sprite2d(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(sprite_texture, sprite_sampler, in.uv);
    let final_color = tex_color * in.tint;
    // Alpha test — discard nearly transparent fragments
    if final_color.a < 0.01 {
        discard;
    }
    return final_color;
}
