// FXAA post pass (ADR 0050) — the cheap interim for the missing MSAA
// (tech-debt #13). Runs AFTER the composite pass: composite renders into an
// intermediate texture in the surface format, this fullscreen pass writes
// the swapchain. Classic FXAA 3.11 quality kernel (Lottes), compact port.
//
// Luma note: the intermediate is *UnormSrgb, so sampling returns LINEAR
// values — FXAA's edge detection needs perceptual luma, so each tap is
// re-encoded via linear_to_srgb before the Rec.601 dot. Output color stays
// linear (FXAA only blends neighbors); the sRGB target does gamma encoding.

struct FxaaUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: FxaaUniforms;

@group(1) @binding(0)
var src_texture: texture_2d<f32>;
@group(1) @binding(1)
var src_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fxaa(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle
    var out: VsOut;
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    return out;
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn perceptual_luma(color: vec3<f32>) -> f32 {
    return dot(linear_to_srgb(color), vec3<f32>(0.299, 0.587, 0.114));
}

const FXAA_REDUCE_MIN: f32 = 1.0 / 128.0;
const FXAA_REDUCE_MUL: f32 = 1.0 / 8.0;
const FXAA_SPAN_MAX: f32 = 8.0;

@fragment
fn fs_fxaa(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let texel = params.texel_size;

    let rgb_m = textureSampleLevel(src_texture, src_sampler, uv, 0.0).rgb;
    let rgb_nw = textureSampleLevel(src_texture, src_sampler, uv + vec2<f32>(-1.0, -1.0) * texel, 0.0).rgb;
    let rgb_ne = textureSampleLevel(src_texture, src_sampler, uv + vec2<f32>(1.0, -1.0) * texel, 0.0).rgb;
    let rgb_sw = textureSampleLevel(src_texture, src_sampler, uv + vec2<f32>(-1.0, 1.0) * texel, 0.0).rgb;
    let rgb_se = textureSampleLevel(src_texture, src_sampler, uv + vec2<f32>(1.0, 1.0) * texel, 0.0).rgb;

    let luma_m = perceptual_luma(rgb_m);
    let luma_nw = perceptual_luma(rgb_nw);
    let luma_ne = perceptual_luma(rgb_ne);
    let luma_sw = perceptual_luma(rgb_sw);
    let luma_se = perceptual_luma(rgb_se);

    let luma_min = min(luma_m, min(min(luma_nw, luma_ne), min(luma_sw, luma_se)));
    let luma_max = max(luma_m, max(max(luma_nw, luma_ne), max(luma_sw, luma_se)));

    var dir = vec2<f32>(
        -((luma_nw + luma_ne) - (luma_sw + luma_se)),
        ((luma_nw + luma_sw) - (luma_ne + luma_se)),
    );

    let dir_reduce = max(
        (luma_nw + luma_ne + luma_sw + luma_se) * 0.25 * FXAA_REDUCE_MUL,
        FXAA_REDUCE_MIN,
    );
    let rcp_dir_min = 1.0 / (min(abs(dir.x), abs(dir.y)) + dir_reduce);
    dir = clamp(
        dir * rcp_dir_min,
        vec2<f32>(-FXAA_SPAN_MAX),
        vec2<f32>(FXAA_SPAN_MAX),
    ) * texel;

    let rgb_a = 0.5 * (
        textureSampleLevel(src_texture, src_sampler, uv + dir * (1.0 / 3.0 - 0.5), 0.0).rgb +
        textureSampleLevel(src_texture, src_sampler, uv + dir * (2.0 / 3.0 - 0.5), 0.0).rgb
    );
    let rgb_b = rgb_a * 0.5 + 0.25 * (
        textureSampleLevel(src_texture, src_sampler, uv + dir * -0.5, 0.0).rgb +
        textureSampleLevel(src_texture, src_sampler, uv + dir * 0.5, 0.0).rgb
    );

    let luma_b = perceptual_luma(rgb_b);
    if (luma_b < luma_min || luma_b > luma_max) {
        return vec4<f32>(rgb_a, 1.0);
    }
    return vec4<f32>(rgb_b, 1.0);
}
