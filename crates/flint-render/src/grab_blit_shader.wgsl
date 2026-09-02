// Grab-pass blit: snapshot the opaque scene color + depth into sampleable
// textures between the pre-ocean and post-ocean passes, so the ocean
// fragment shader can refract what's underwater (the player's legs) and
// apply Beer-Lambert turbidity by water-column depth.
//
// Reading via textureLoad (no samplers, no filterable-format concerns);
// depth is written out as R32Float color.

@group(0) @binding(0) var scene_color: texture_2d<f32>;
@group(0) @binding(1) var scene_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

struct BlitOut {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
};

@fragment
fn fs_blit(in: VsOut) -> BlitOut {
    let pixel = vec2<i32>(in.position.xy);
    var out: BlitOut;
    out.color = textureLoad(scene_color, pixel, 0);
    out.depth = textureLoad(scene_depth, pixel, 0);
    return out;
}
