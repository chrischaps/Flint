// Bloom shader — threshold, downsample, and upsample passes
//
// Based on the Call of Duty: Advanced Warfare technique:
// - Threshold: extract bright pixels with soft knee
// - Downsample: 13-tap filter for anti-aliased progressive reduction
// - Upsample: 9-tap tent filter for smooth reconstruction

struct BloomUniforms {
    texel_size: vec2<f32>,
    threshold: f32,
    soft_threshold: f32,
};

@group(0) @binding(0)
var<uniform> params: BloomUniforms;

@group(1) @binding(0)
var src_texture: texture_2d<f32>;
@group(1) @binding(1)
var src_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bloom(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Soft threshold with knee: smooth transition instead of hard cutoff
fn soft_threshold_weight(brightness: f32, threshold: f32, knee: f32) -> f32 {
    let soft = brightness - threshold + knee;
    let soft_clamped = clamp(soft, 0.0, 2.0 * knee);
    let contribution = soft_clamped * soft_clamped / (4.0 * knee + 0.00001);
    return max(contribution, brightness - threshold) / max(brightness, 0.00001);
}

// Threshold pass: extract bright pixels
@fragment
fn fs_bloom_threshold(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(src_texture, src_sampler, in.uv).rgb;
    let brightness = max(color.r, max(color.g, color.b));
    let knee = params.threshold * params.soft_threshold;
    let weight = soft_threshold_weight(brightness, params.threshold, knee);
    return vec4<f32>(color * weight, 1.0);
}

// Downsample pass: 13-tap filter (Jimenez 2014 / CoD:AW)
// Uses a box filter pattern that avoids aliasing artifacts
@fragment
fn fs_downsample(in: VsOut) -> @location(0) vec4<f32> {
    let ts = params.texel_size;

    // Center sample
    let a = textureSample(src_texture, src_sampler, in.uv).rgb;

    // Inner box (4 samples at half-pixel offsets)
    let b = textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-0.5, -0.5) * ts).rgb;
    let c = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.5, -0.5) * ts).rgb;
    let d = textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-0.5,  0.5) * ts).rgb;
    let e = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.5,  0.5) * ts).rgb;

    // Outer box (8 samples at full-pixel offsets)
    let f = textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0, -1.0) * ts).rgb;
    let g = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.0, -1.0) * ts).rgb;
    let h = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0, -1.0) * ts).rgb;
    let i = textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0,  0.0) * ts).rgb;
    let j = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0,  0.0) * ts).rgb;
    let k = textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0,  1.0) * ts).rgb;
    let l = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.0,  1.0) * ts).rgb;
    let m = textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0,  1.0) * ts).rgb;

    // Weighted average: center box gets 0.5, outer corners get the rest
    var color = a * 0.125;
    color += (b + c + d + e) * 0.125;
    color += (f + h + k + m) * 0.03125;
    color += (g + i + j + l) * 0.0625;

    return vec4<f32>(color, 1.0);
}

// Upsample pass: 9-tap tent filter (3x3 bilinear)
// Produces a smooth, wide blur during the upsample chain
@fragment
fn fs_upsample(in: VsOut) -> @location(0) vec4<f32> {
    let ts = params.texel_size;

    var color = vec3<f32>(0.0);

    // 3x3 tent filter weights: 1/16 for corners, 2/16 for edges, 4/16 for center
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0, -1.0) * ts).rgb * (1.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.0, -1.0) * ts).rgb * (2.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0, -1.0) * ts).rgb * (1.0 / 16.0);

    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0,  0.0) * ts).rgb * (2.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.0,  0.0) * ts).rgb * (4.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0,  0.0) * ts).rgb * (2.0 / 16.0);

    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>(-1.0,  1.0) * ts).rgb * (1.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 0.0,  1.0) * ts).rgb * (2.0 / 16.0);
    color += textureSample(src_texture, src_sampler, in.uv + vec2<f32>( 1.0,  1.0) * ts).rgb * (1.0 / 16.0);

    return vec4<f32>(color, 1.0);
}
