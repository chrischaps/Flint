// Depth resolve for MSAA (ADR 0058): copy sample 0 of the multisampled
// scene depth into a single-sample depth target via a fullscreen triangle
// writing frag_depth. Sample 0 is the industry-standard choice — averaging
// depth is wrong at silhouettes, and every depth consumer (SSAO, DoF, fog,
// volumetric, ocean grab) then stays single-sample and unchanged.

@group(0) @binding(0)
var msaa_depth: texture_depth_multisampled_2d;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

struct FsOut {
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_resolve(in: VsOut) -> FsOut {
    let coords = vec2<i32>(in.position.xy);
    var out: FsOut;
    out.depth = textureLoad(msaa_depth, coords, 0);
    return out;
}
