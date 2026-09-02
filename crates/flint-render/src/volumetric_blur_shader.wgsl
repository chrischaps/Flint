// Bilateral blur for volumetric lighting
//
// 5x5 depth-aware blur at half resolution. Rejects samples across large depth
// discontinuities to prevent god-ray light bleeding across edges.

struct BlurUniforms {
    texel_size: vec2<f32>,
    depth_threshold: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> params: BlurUniforms;

@group(1) @binding(0)
var source_texture: texture_2d<f32>;
@group(1) @binding(1)
var source_sampler: sampler;

@group(1) @binding(2)
var depth_texture: texture_2d<f32>;
@group(1) @binding(3)
var depth_sampler_nn: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_volumetric_blur(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Linearize a reverse-Z depth value
fn linearize_depth(d: f32) -> f32 {
    // For reverse-Z with infinite far: linear_z ≈ near / depth
    // For finite far: use the standard formula
    return d;
}

@fragment
fn fs_volumetric_blur(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let center_depth = textureSample(depth_texture, depth_sampler_nn, uv).r;
    let center_color = textureSample(source_texture, source_sampler, uv).rgb;

    // 5x5 Gaussian-ish weights
    let kernel_size = 2;
    var total_weight = 0.0;
    var result = vec3<f32>(0.0);

    for (var y = -kernel_size; y <= kernel_size; y = y + 1) {
        for (var x = -kernel_size; x <= kernel_size; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * params.texel_size;
            let sample_uv = uv + offset;
            let sample_color = textureSample(source_texture, source_sampler, sample_uv).rgb;
            let sample_depth = textureSample(depth_texture, depth_sampler_nn, sample_uv).r;

            // Depth-aware weighting: reject across discontinuities
            let depth_diff = abs(center_depth - sample_depth);
            let depth_weight = step(depth_diff, params.depth_threshold);

            // Spatial Gaussian weight
            let dist = f32(x * x + y * y);
            let spatial_weight = exp(-dist * 0.2);

            let w = spatial_weight * depth_weight;
            result = result + sample_color * w;
            total_weight = total_weight + w;
        }
    }

    if (total_weight > 0.0) {
        result = result / total_weight;
    } else {
        result = center_color;
    }

    return vec4<f32>(result, 1.0);
}
