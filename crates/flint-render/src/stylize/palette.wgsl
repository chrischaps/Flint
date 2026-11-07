// Palette quantize (#324): the finished picture in a palette's colours and
// nothing else. No glyphs: each block of `block` pixels takes the average
// of its pixels and is matched against the palette's table, ground
// included. A nearest palette goes through the shared matcher
// (src/palette.wgsl, concatenated before this file: `pal_pick` in OKLab,
// as ASCII matches); a ladder is climbed here by the block's luminance.
// The ordered dither is fixed to the block grid, so a still camera gives a
// still pattern.

struct PaletteUniforms {
    len: u32,
    matching: u32,
    srgb: u32,
    block: u32,
    dither: f32,
    chroma: f32,
    _pad0: f32,
    _pad1: f32,
    // The table: the inks with the ground in its place, display values,
    // and the same colours in OKLab
    inks: array<vec4<f32>, 33>,
    lab: array<vec4<f32>, 33>,
};

@group(0) @binding(0) var<uniform> u: PaletteUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn palette_ink(i: u32) -> vec3<f32> {
    return u.inks[i].rgb;
}

fn palette_ink_lab(i: u32) -> vec3<f32> {
    return u.lab[i].rgb;
}

// The block's average display colour. One pixel is read as it is; a
// larger block takes bilinear taps on pixel corners, each the mean of a
// 2x2 quad: one tap is all of a 2x2 block, four are all of a 4x4 one, and a
// larger block averages four quads spread across it. On an sRGB target
// the filter mixes linear light, as the eye would blur it.
fn block_color(cell: vec2<i32>) -> vec3<f32> {
    let b = i32(max(u.block, 1u));
    var c: vec3<f32>;
    if b == 1 {
        let dims = vec2<i32>(textureDimensions(input_texture));
        c = textureLoad(input_texture, clamp(cell, vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb;
    } else {
        let texel = 1.0 / vec2<f32>(textureDimensions(input_texture));
        let n = select(1, 2, b >= 4);
        let origin = vec2<f32>(cell * b);
        c = vec3<f32>(0.0);
        for (var y = 0; y < n; y++) {
            for (var x = 0; x < n; x++) {
                // Corners at b/2 (one tap) or b/4 and 3b/4 (four), rounded
                // to whole pixels so each tap meets four pixel centres
                let at = floor(vec2<f32>(f32(2 * x + 1), f32(2 * y + 1)) * f32(b) / f32(2 * n) + 0.5);
                c += textureSampleLevel(input_texture, input_sampler, (origin + at) * texel, 0.0).rgb;
            }
        }
        c /= f32(n * n);
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let b = i32(max(u.block, 1u));
    let cell = vec2<i32>(floor(pos.xy)) / b;
    let col = block_color(cell);
    let n = max(u.len, 1u);
    var ink: vec3<f32>;
    if u.matching == 1u {
        // A ladder: the luminance's rung, moved up to half a rung either
        // way by the dither (Palette::quantize)
        let t = clamp(dot(col, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
        let td = clamp(t + (pal_bayer(cell) - 0.5) * u.dither / f32(n), 0.0, 1.0);
        ink = palette_ink(min(u32(td * f32(n)), n - 1u));
    } else {
        ink = pal_pick(0u, n, col, u.chroma, u.dither, cell);
    }
    return vec4<f32>(select(ink, to_linear(ink), u.srgb == 1u), 1.0);
}
