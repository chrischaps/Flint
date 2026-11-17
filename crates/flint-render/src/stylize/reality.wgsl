// The reality styles' prelude (#746, stylize/reality.rs): what matrix,
// blood, drunk, tron and underwater share. tear_mask.wgsl's noise (hash2,
// value_noise, fbm) comes before it; each style's own file after it, with
// its `fs_main`.
//
// They were render modes of the composite, drawn on its tone-mapped colour
// before the vignette, grain and dither. As display styles they read the
// finished picture, so `picture` hands them its linear colour with the
// vignette taken out, and `shown` puts the vignette back on what they draw:
// the look lands where the composite drew it.

struct Reality {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    time: f32,
    texel_size: vec2<f32>,
    near: f32,
    far: f32,
    // 1 on an sRGB target (samples and writes are linear), the frame's
    // vignette intensity and smoothness, width / height
    srgb: f32,
    vignette: f32,
    vignette_smoothness: f32,
    aspect: f32,
    // The style's own numbers (each style's file says which is which)
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
};

@group(0) @binding(0) var<uniform> r: Reality;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var depth_texture: texture_2d<f32>;
@group(2) @binding(1) var depth_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

// The composite's vignette at `uv`: what it multiplied the colour by.
fn vignette_at(uv: vec2<f32>) -> f32 {
    if (r.vignette <= 0.0) {
        return 1.0;
    }
    let vdist = length(uv - vec2<f32>(0.5, 0.5)) * 1.41421356;
    return max(1.0 - pow(vdist, r.vignette_smoothness) * r.vignette, 0.0);
}

// The picture's linear colour at `uv`, as the input holds it.
fn sample_linear(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(input_texture, input_sampler, uv, 0.0).rgb;
    return select(flint_to_linear(c), c, r.srgb > 0.5);
}

// The picture at `uv` as the composite had it before its vignette.
fn picture(uv: vec2<f32>) -> vec3<f32> {
    return sample_linear(uv) / max(vignette_at(uv), 1e-3);
}

// What a style writes for linear colour `c` at `uv`: the vignette put back,
// then encoded for a target that isn't sRGB.
fn shown(c: vec3<f32>, uv: vec2<f32>) -> vec4<f32> {
    let v = c * vignette_at(uv);
    return vec4<f32>(select(flint_to_display(v), v, r.srgb > 0.5), 1.0);
}

// The scene's depth at `uv` (0 near, 1 far; the sky is 1).
fn depth_at(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(depth_texture, depth_sampler, uv, 0.0).r;
}

fn is_sky_depth(depth: f32) -> bool {
    return depth >= 1.0;
}

// Linearize a [0,1] depth to view depth (wgpu convention, ADR 0055).
fn linear_depth(depth: f32) -> f32 {
    return r.near * r.far / (r.far - depth * (r.far - r.near));
}

// World position from a uv and its depth.
fn world_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    return flint_unproject(r.inv_view_proj, uv, depth);
}
