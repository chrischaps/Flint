// Depth upscale for render scale (#122): the scene's depth, drawn below the
// surface's size, copied into the surface-sized depth view via a fullscreen
// triangle writing frag_depth. Nearest texel, like the MSAA resolve's
// sample 0: blending depth across a silhouette would invent surfaces, and
// the overlays tested against it (world lines, world text) only need where
// the scene is.

@group(0) @binding(0)
var scene_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    return out;
}

struct FsOut {
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_upscale(in: VsOut) -> FsOut {
    let size = vec2<f32>(textureDimensions(scene_depth));
    let coords = min(vec2<i32>(in.uv * size), vec2<i32>(size) - vec2<i32>(1));
    var out: FsOut;
    out.depth = textureLoad(scene_depth, coords, 0);
    return out;
}
