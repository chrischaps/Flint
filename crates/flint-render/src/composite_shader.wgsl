// Composite post-processing shader
//
// Fullscreen triangle that reads from the HDR scene buffer, applies
// radial blur, chromatic aberration, bloom, fog, exposure, ACES tonemapping,
// gamma correction, and vignette, then writes to the sRGB surface.

struct PostProcessUniforms {
    exposure: f32,
    bloom_intensity: f32,
    bloom_threshold: f32,
    bloom_soft_threshold: f32,
    vignette_intensity: f32,
    vignette_smoothness: f32,
    texel_size: vec2<f32>,
    chromatic_aberration: f32,
    radial_blur: f32,
    _pad: vec2<f32>,
    // Fog parameters
    fog_color: vec3<f32>,
    fog_density: f32,
    fog_start: f32,
    fog_end: f32,
    fog_height_falloff: f32,
    fog_height_origin: f32,
    camera_pos: vec3<f32>,
    fog_enabled: f32,
    near: f32,
    far: f32,
    fog_height_enabled: f32,
    _pad2: f32,
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> params: PostProcessUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

@group(2) @binding(0)
var bloom_texture: texture_2d<f32>;
@group(2) @binding(1)
var bloom_sampler: sampler;

@group(3) @binding(0)
var ssao_texture: texture_2d<f32>;
@group(3) @binding(1)
var ssao_sampler: sampler;

@group(3) @binding(2)
var depth_texture: texture_2d<f32>;
@group(3) @binding(3)
var depth_sampler_nn: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_composite(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle: 3 vertices cover the whole screen
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    // Map clip space to UV: [-1,1] -> [0,1], flip Y
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// ACES filmic tone mapping curve
fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

// Linear to sRGB gamma correction
fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2));
}

// Reconstruct world position from depth buffer UV + depth value
fn world_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let clip = vec4<f32>(uv * 2.0 - 1.0, depth, 1.0);
    // wgpu clip space has Y flipped relative to UV space
    let clip_y_flip = vec4<f32>(clip.x, -clip.y, clip.z, 1.0);
    let world = params.inv_view_proj * clip_y_flip;
    return world.xyz / world.w;
}

// Compute fog factor from depth buffer
fn compute_fog(uv: vec2<f32>) -> f32 {
    let depth = textureSample(depth_texture, depth_sampler_nn, uv).r;
    // Skip skybox (depth at or near far plane)
    if (depth >= 0.9999) {
        return 0.0;
    }

    let world_pos = world_pos_from_depth(uv, depth);
    let dist = length(world_pos - params.camera_pos);

    // Exponential distance fog with start offset
    let dist_factor = max(dist - params.fog_start, 0.0);
    let dist_fog = 1.0 - exp(-params.fog_density * dist_factor);

    // Height fog (exponential falloff above fog_height_origin)
    var height_fog = 0.0;
    if (params.fog_height_enabled > 0.5) {
        let height_above = max(world_pos.y - params.fog_height_origin, 0.0);
        height_fog = exp(-params.fog_height_falloff * height_above);
        // Modulate by distance so nearby objects aren't fully fogged
        height_fog = height_fog * dist_fog;
    }

    return clamp(max(dist_fog, height_fog), 0.0, 1.0);
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let center = vec2<f32>(0.5, 0.5);
    let dir_from_center = uv - center;
    let dist = length(dir_from_center);

    // ── Radial Blur (8-tap, center stays sharp, edges blur) ──
    var color: vec3<f32>;
    if (params.radial_blur > 0.001) {
        let blur_str = params.radial_blur * dist * 0.04;
        let blur_dir = normalize(dir_from_center + vec2<f32>(0.0001, 0.0001)) * blur_str;
        var acc = vec3<f32>(0.0);
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -3.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -2.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -1.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir * -0.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  0.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  1.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  2.5).rgb;
        acc += textureSample(hdr_texture, hdr_sampler, uv + blur_dir *  3.5).rgb;
        let blurred = acc / 8.0;
        let blur_mix = smoothstep(0.1, 0.7, dist) * params.radial_blur;
        color = mix(textureSample(hdr_texture, hdr_sampler, uv).rgb, blurred, clamp(blur_mix, 0.0, 1.0));
    } else {
        color = textureSample(hdr_texture, hdr_sampler, uv).rgb;
    }

    // ── Chromatic Aberration (R/B offset radially, G stays) ──
    if (params.chromatic_aberration > 0.001) {
        let offset = dir_from_center * params.chromatic_aberration * 0.012;
        let r = textureSample(hdr_texture, hdr_sampler, uv + offset).r;
        let b = textureSample(hdr_texture, hdr_sampler, uv - offset).b;
        color = vec3<f32>(r, color.g, b);
    }

    // ── SSAO ──
    let ao = textureSample(ssao_texture, ssao_sampler, uv).r;
    color = color * ao;

    // ── Bloom ──
    let bloom = textureSample(bloom_texture, bloom_sampler, uv).rgb;
    color = color + bloom * params.bloom_intensity;

    // ── Fog (applied in linear HDR space, before tonemapping) ──
    if (params.fog_enabled > 0.5) {
        let fog_factor = compute_fog(uv);
        color = mix(color, params.fog_color, fog_factor);
    }

    // ── Exposure → Tonemapping → Gamma ──
    color = color * params.exposure;
    let mapped = aces_filmic(color);
    var corrected = linear_to_srgb(mapped);

    // ── Vignette ──
    if (params.vignette_intensity > 0.0) {
        let vdist = dist * 1.41421356;
        let vignette = 1.0 - pow(vdist, params.vignette_smoothness) * params.vignette_intensity;
        corrected = corrected * max(vignette, 0.0);
    }

    return vec4<f32>(corrected, 1.0);
}
