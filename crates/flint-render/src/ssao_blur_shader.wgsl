// SSAO bilateral blur shader
//
// Applies a 4x4 box blur to smooth the noisy SSAO output.
// The noise texture tiles at 4x4, so a 4x4 blur cleanly averages it out.

struct BlurUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: BlurUniforms;

@group(1) @binding(0)
var ssao_texture: texture_2d<f32>;
@group(1) @binding(1)
var ssao_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_ssao_blur(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

@fragment
fn fs_ssao_blur(in: VsOut) -> @location(0) f32 {
    let uv = in.uv;
    var result = 0.0;

    // 4x4 box blur centered on the pixel
    for (var x = -2; x < 2; x++) {
        for (var y = -2; y < 2; y++) {
            let offset = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5) * params.texel_size;
            result += textureSample(ssao_texture, ssao_sampler, uv + offset).r;
        }
    }

    return result / 16.0;
}
