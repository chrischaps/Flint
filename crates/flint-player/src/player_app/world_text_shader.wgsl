// Depth-tested world text (ADR 0104).
//
// Glyph quads from egui's tessellator, already in clip space, sampling
// egui's font atlas. The colour maths is egui-wgpu's for a gamma
// framebuffer (the HUD's non-sRGB view), so the text matches HUD text.

struct VsIn {
    @location(0) position: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: u32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>, // premultiplied, gamma 0-1
};

@group(0) @binding(0) var r_tex_color: texture_2d<f32>;
@group(0) @binding(1) var r_tex_sampler: sampler;

// [u8; 4] sRGB as u32 -> [r, g, b, a] in 0-1
fn unpack_color(color: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(color & 255u),
        f32((color >> 8u) & 255u),
        f32((color >> 16u) & 255u),
        f32((color >> 24u) & 255u),
    ) / 255.0;
}

// 0-1 sRGB gamma from 0-1 linear
fn gamma_from_linear_rgb(rgb: vec3<f32>) -> vec3<f32> {
    let cutoff = rgb < vec3<f32>(0.0031308);
    let lower = rgb * vec3<f32>(12.92);
    let higher = vec3<f32>(1.055) * pow(rgb, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, cutoff);
}

@vertex
fn vs_world_text(v: VsIn) -> VsOut {
    var out: VsOut;
    out.position = v.position;
    out.uv = v.uv;
    out.color = unpack_color(v.color);
    return out;
}

@fragment
fn fs_world_text(in: VsOut) -> @location(0) vec4<f32> {
    // The atlas is an sRGB texture, so the sample comes back linear.
    let tex_linear = textureSample(r_tex_color, r_tex_sampler, in.uv);
    let tex_gamma = vec4<f32>(gamma_from_linear_rgb(tex_linear.rgb), tex_linear.a);
    return in.color * tex_gamma;
}
