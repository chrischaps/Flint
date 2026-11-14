// Depth-tested world text (ADR 0104).
//
// Glyph quads from egui's tessellator, already in clip space, sampling
// egui's font atlas (and a halo's picture). The colour maths is egui-wgpu's
// for a gamma framebuffer (the HUD's non-sRGB view), so the text matches HUD
// text. Since egui 0.31 egui-wgpu's textures are Rgba8Unorm, not sRGB, so a
// sample is already gamma: decoding it again (as 0.30's sRGB textures
// needed) paled every glow and stroke picture and fattened glyph edges in
// world text (#949).

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
    // egui-wgpu's fs_main_gamma_framebuffer, less its dither: the texture
    // is Rgba8Unorm, premultiplied gamma, as the vertex colour is.
    return in.color * textureSample(r_tex_color, r_tex_sampler, in.uv);
}
