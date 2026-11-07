// ASCII (#265): what both of its passes share. The cell pass
// (ascii_cells.wgsl) decides each cell's colour and glyph, one texel a
// cell, remembering last frame's (#277); the glyph pass (ascii.wgsl) draws
// the glyphs it chose at output resolution.
//
// On an sRGB target the input samples arrive linear and the output is
// encoded on the way out, so the passes decode to display values, work
// there (a ramp is a ladder of what the eye sees), and decode back.

// One per-entity variant's settings (#266), drawn per pixel in the glyph
// pass from its own samples.
struct Variant {
    cell: vec2<f32>,
    origin: vec2<f32>,
    tint: vec3<f32>,
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    ramp_start: u32,
    ramp_len: u32,
    // 1: leave these pixels as they are
    flags: u32,
    glow: f32,
    ground: u32,
    palette_start: u32,
    palette_bg: vec3<f32>,
    palette_len: u32,
    palette_match: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

struct AsciiUniforms {
    // Cell size in output pixels, and where the grid starts (centred, so
    // the part cells at the borders are the same on both sides).
    cell: vec2<f32>,
    origin: vec2<f32>,
    // The atlas's glyph cell, in atlas pixels.
    glyph: vec2<f32>,
    size: vec2<f32>,
    tint: vec3<f32>,
    // 0 source colour, 1 tint, 2 raw cell colour, 3 mono, 4 palette
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    ramp_len: u32,
    srgb: u32,
    // Halo strength around glyphs (0 off)
    glow: f32,
    // 0 dim (the mode's own), 1 black, 2 inverse (ink fills, glyph cut out)
    ground: u32,
    palette_len: u32,
    // 0 nearest ink to the cell's hue, 1 a ladder picked by tone
    palette_match: u32,
    palette_bg: vec3<f32>,
    _pad: u32,
    // Temporal stability (#277): how much of last frame's cell colour a
    // cell keeps, how far (in ramp steps) its tone may stray before its
    // glyph changes, the ordered dither's strength, and whether last
    // frame's cells are this picture's past (0 on a first frame).
    smoothing: f32,
    hysteresis: f32,
    dither: f32,
    history: u32,
    // Line glyphs' strength over the tone glyph (#267), and whether the
    // edge passes ran
    edges: f32,
    edge_on: u32,
    _pad_edges0: u32,
    _pad_edges1: u32,
    // The edge glyphs' atlas origins, horizontal, vertical, diagonal,
    // antidiagonal, two per vec4 like the ramp
    edge_glyphs: array<vec4<u32>, 2>,
    // Each ramp step's atlas cell origin, two steps per vec4: (x0, y0, x1, y1).
    ramp: array<vec4<u32>, 64>,
    // Every variant's palette inks, display values (rgb, unused w); the
    // whole frame's first
    palette: array<vec4<f32>, 32>,
    // Per-entity variants (#266). x: rows in use (1 = the whole frame
    // alone); y: 1 when the whole frame passes through (only entities
    // asked). Row 0 is unused: the whole frame draws from the fields above
    // and the cell state.
    variant_meta: vec4<u32>,
    variants: array<Variant, 64>,
    // The variant slot of each mask id below 256, four ids a vec4.
    slots: array<vec4<u32>, 64>,
};

@group(0) @binding(0) var<uniform> u: AsciiUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
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

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Luminance remapped to the ramp: contrast about mid-grey, then brightness.
fn tone(lum: f32) -> f32 {
    return clamp((lum - 0.5) * u.contrast + 0.5 + u.brightness, 0.0, 1.0);
}

// A tone after the optional tone bands (`levels`).
fn banded(tone_in: f32) -> f32 {
    let t = clamp(tone_in, 0.0, 1.0);
    return select(t, round(t * u.levels) / u.levels, u.levels >= 1.0);
}

// The cell state texture is one texel a cell, offset by one so the part
// cell left of (above) the origin is texel 0.
const CELL_OFFSET: i32 = 1;
// Cell colour channels are stored as 16-bit unorm in a uint texture.
const UNORM16: f32 = 65535.0;
