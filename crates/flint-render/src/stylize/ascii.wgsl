// ASCII (#265): the full-screen, grid-aligned glyph style.
//
// Runs at the display stage, on the finished picture at output resolution,
// so the grid stays crisp under a render scale. Every cell of one screen
// grid becomes one glyph from the atlas (#264): the cell's luminance picks a
// step of the ramp, and the ink takes the cell's colour or a tint.
//
// On an sRGB target the input samples arrive linear and the output is
// encoded on the way out, so the pass decodes to display values, works
// there (a ramp is a ladder of what the eye sees), and decodes back.
//
// Per entity (#266): each pixel's style mask id picks a variant, a row of
// settings of its own (cell, grid origin, ramp, colour). Slot 0 is the
// whole frame's; it passes the picture through when only entities asked
// for the style. A cell's colour averages only the lattice samples of the
// pixel's own variant, so a styled prop's glyphs carry its colour and not
// the background's, and its silhouette is the mask's, to the pixel.

struct Variant {
    // Cell size in output pixels, and where the grid starts (centred on
    // the frame, or pinned to an entity's projected origin).
    cell: vec2<f32>,
    origin: vec2<f32>,
    tint: vec3<f32>,
    // 0 source colour, 1 tint, 2 raw cell colour
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    // This variant's steps in `ramp`
    ramp_start: u32,
    ramp_len: u32,
    // 1: leave these pixels as they are
    flags: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

struct AsciiUniforms {
    // The atlas's glyph cell, in atlas pixels.
    glyph: vec2<f32>,
    size: vec2<f32>,
    srgb: u32,
    variant_count: u32,
    _p0: u32,
    _p1: u32,
    variants: array<Variant, 64>,
    // The variant slot of each mask id below 256, four ids a vec4.
    slots: array<vec4<u32>, 64>,
    // Each ramp step's atlas cell origin, two steps per vec4: (x0, y0, x1, y1).
    ramp: array<vec4<u32>, 64>,
};

@group(0) @binding(0) var<uniform> u: AsciiUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var atlas: texture_2d<f32>;
@group(3) @binding(0) var style_mask: texture_2d<u32>;

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

fn shown(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let c = clamp(textureLoad(input_texture, clamp(p, vec2<i32>(0), max_px), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

// The variant slot of the pixel at `p` (output pixels): the mask is at the
// scene's resolution, so it is read by position scaled to it.
fn slot_at(p: vec2<f32>) -> u32 {
    if u.variant_count <= 1u {
        return 0u;
    }
    let msize = vec2<i32>(textureDimensions(style_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    let id = textureLoad(style_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r;
    if id >= 256u {
        return 0u;
    }
    return u.slots[id / 4u][id % 4u];
}

fn ramp_origin(step: u32) -> vec2<u32> {
    let pair = u.ramp[step / 2u];
    return select(pair.xy, pair.zw, (step & 1u) == 1u);
}

// The glyph's coverage under the pixel at `local` (pixel centre, in the
// cell), from the glyph cell at `origin` in the atlas. Sharp bilinear: at
// a whole-number scale (8 px glyphs in 8, 16 or 24 px cells) it is exactly
// nearest; at a fractional one (12 px, a 1.5x display) each atlas pixel
// keeps one width and only its border pixels blend, so strokes don't
// alternate 1 and 2 px. The atlas's one-pixel gutter is empty, so the
// neighbours past a glyph's edge read as no ink.
fn glyph_coverage(origin: vec2<u32>, local: vec2<f32>, cell_size: vec2<f32>) -> f32 {
    let scale = cell_size / u.glyph;
    let gp = local / scale - 0.5;
    let base = floor(gp);
    let f = clamp((gp - base - 0.5) * scale + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    let o = vec2<i32>(origin);
    let hi = vec2<i32>(u.glyph);
    let b = vec2<i32>(base);
    let i0 = clamp(b, vec2<i32>(-1), hi);
    let i1 = clamp(b + vec2<i32>(1), vec2<i32>(-1), hi);
    let c00 = textureLoad(atlas, o + vec2<i32>(i0.x, i0.y), 0).r;
    let c10 = textureLoad(atlas, o + vec2<i32>(i1.x, i0.y), 0).r;
    let c01 = textureLoad(atlas, o + vec2<i32>(i0.x, i1.y), 0).r;
    let c11 = textureLoad(atlas, o + vec2<i32>(i1.x, i1.y), 0).r;
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let slot = slot_at(p);
    // A constant index while only the whole frame's row is in use: a
    // dynamically indexed uniform load costs the full-screen case ~50%
    var v: Variant;
    if u.variant_count <= 1u {
        v = u.variants[0];
    } else {
        v = u.variants[slot];
    }
    if (v.flags & 1u) != 0u {
        // Not this style's pixel: the picture as it came
        return textureLoad(input_texture, vec2<i32>(floor(p)), 0);
    }
    let rel = p - v.origin;
    let cell = floor(rel / v.cell);
    let cell_min = cell * v.cell + v.origin;
    let local = rel - cell * v.cell;

    // The cell's colour: the mean of a 4x4 lattice over a footprint
    // `footprint` cells wide, centred on the cell, counting only the
    // samples of this pixel's own variant
    let foot = v.cell * max(v.footprint, 1.0);
    let centre = cell_min + 0.5 * v.cell;
    var sum = vec3<f32>(0.0);
    var count = 0.0;
    if u.variant_count <= 1u {
        // The whole frame alone: every sample counts. Kept apart so its 16
        // loads issue back to back; under a per-sample branch they ran
        // ~50% slower
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let q = centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot;
                sum += shown(vec2<i32>(floor(q)));
            }
        }
        count = 16.0;
    } else {
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let q = floor(centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot);
                let w = select(0.0, 1.0, slot_at(q) == slot);
                sum += shown(vec2<i32>(q)) * w;
                count += w;
            }
        }
    }
    let col = select(shown(vec2<i32>(floor(p))), sum / max(count, 1.0), count > 0.0);

    // Luminance, remapped: contrast about mid-grey, then brightness
    let lum = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    var t = clamp((lum - 0.5) * v.contrast + 0.5 + v.brightness, 0.0, 1.0);
    if v.levels >= 1.0 {
        t = round(t * v.levels) / v.levels;
    }
    let n = max(v.ramp_len, 1u);
    let step = min(u32(t * f32(n)), n - 1u);

    let coverage = glyph_coverage(ramp_origin(v.ramp_start + step), local, v.cell);

    // Ink and ground
    let hue = col / max(max(col.r, max(col.g, col.b)), 1e-4);
    var ink = vec3<f32>(0.0);
    var ground = col * v.background;
    // Density carries brightness and the ink carries hue at full value, so
    // a dark object's sparse glyphs still read as its colour
    switch v.color_mode {
        case 1u: {
            ink = v.tint;
            ground = v.tint * lum * v.background;
        }
        case 2u: {
            ink = col;
        }
        default: {
            ink = hue;
        }
    }
    ink = clamp(ink * v.ink, vec3<f32>(0.0), vec3<f32>(1.0));
    let out = mix(ground, ink, coverage);
    return vec4<f32>(select(out, to_linear(out), u.srgb == 1u), 1.0);
}
