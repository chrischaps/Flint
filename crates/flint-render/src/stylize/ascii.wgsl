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
    // The palette's inks, display values (rgb, unused w)
    palette: array<vec4<f32>, 32>,
};

@group(0) @binding(0) var<uniform> u: AsciiUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var atlas: texture_2d<f32>;
// One texel a cell (offset by one), from the cell pass: x = edge glyph
// slot + 1, 0 for none (ascii_cells.wgsl)
@group(3) @binding(0) var cells: texture_2d<f32>;

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
fn glyph_coverage(origin: vec2<u32>, local: vec2<f32>) -> f32 {
    let scale = u.cell / u.glyph;
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

// The palette ink for a cell of full-value `hue` and tone `t`
fn palette_ink(hue: vec3<f32>, t: f32) -> vec3<f32> {
    let n = max(u.palette_len, 1u);
    if u.palette_match == 1u {
        return u.palette[min(u32(t * f32(n)), n - 1u)].rgb;
    }
    var best = u.palette[0].rgb;
    var best_d = 1e9;
    for (var i = 0u; i < n; i++) {
        let c = u.palette[i].rgb;
        let d = dot(c - hue, c - hue);
        if d < best_d {
            best_d = d;
            best = c;
        }
    }
    return best;
}

// How much of the glyph lies within a couple of pixels of `local`, less
// what's under it: the halo a bright glyph throws on its own cell.
fn glyph_halo(origin: vec2<u32>, local: vec2<f32>, coverage: f32) -> f32 {
    let r = max(u.cell.x / u.glyph.x, 1.0) * 1.25;
    var sum = 0.0;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * 0.7853982;
        let d = vec2<f32>(cos(a), sin(a));
        sum += glyph_coverage(origin, local + d * r);
        sum += 0.5 * glyph_coverage(origin, local + d * r * 2.0);
    }
    return max(sum / 12.0 - coverage, 0.0);
}

// The cell pass's texel for cell texel `c`: (slot + 1, length, outline).
fn cell_at(c: vec2<i32>) -> vec4<f32> {
    let cmax = vec2<i32>(textureDimensions(cells)) - vec2<i32>(1);
    return textureLoad(cells, clamp(c, vec2<i32>(0), cmax), 0);
}

// Whether neighbour `n` takes the line from a cell with `slot` and `len`:
// the same glyph, and more of the line (or as much, on the earlier side).
fn yields_to(n: vec4<f32>, slot: u32, len: f32, earlier: bool) -> bool {
    if n.x < 0.5 || min(u32(n.x - 0.5), 3u) != slot {
        return false;
    }
    return n.y > len || (n.y == len && earlier);
}

// The edge glyph slot of cell texel `c`, or 4 for none. A line running
// near a cell border splits between the two cells, and each would draw it:
// a cell holding less than one cell width of line yields to a neighbour
// across the line (left/right of `│`, above/below of `─`, either side of a
// diagonal) holding more. A full crossing always draws, so two real edges
// a cell apart both stay.
fn edge_slot(c: vec2<i32>) -> u32 {
    let e = cell_at(c);
    if e.x < 0.5 {
        return 4u;
    }
    let slot = min(u32(e.x - 0.5), 3u);
    let len = e.y;
    if len >= 1.0 {
        return slot;
    }
    // Offsets to the neighbours on the line normal's positive side (screen
    // y down); the negative side is the same offsets negated, and wins
    // ties. `─` and `│` have one; a diagonal three, since a staircase of
    // `╱` doubles by a straight step as often as by a diagonal one.
    var off = array<vec2<i32>, 3>(vec2<i32>(0, 1), vec2<i32>(0, 1), vec2<i32>(0, 1));
    switch slot {
        case 1u: {
            off = array<vec2<i32>, 3>(vec2<i32>(1, 0), vec2<i32>(1, 0), vec2<i32>(1, 0));
        }
        case 2u: {
            // `╱` runs (1, -1): its normal is (1, 1)
            off = array<vec2<i32>, 3>(vec2<i32>(1, 1), vec2<i32>(1, 0), vec2<i32>(0, 1));
        }
        case 3u: {
            // `╲` runs (1, 1): its normal is (1, -1)
            off = array<vec2<i32>, 3>(vec2<i32>(1, -1), vec2<i32>(1, 0), vec2<i32>(0, -1));
        }
        default: {}
    }
    for (var k = 0; k < 3; k++) {
        if yields_to(cell_at(c - off[k]), slot, len, true)
            || yields_to(cell_at(c + off[k]), slot, len, false) {
            return 4u;
        }
    }
    return slot;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let rel = p - u.origin;
    let cell = floor(rel / u.cell);
    let cell_min = cell * u.cell + u.origin;
    let local = rel - cell * u.cell;

    // The cell's colour: the mean of a 4x4 lattice over a footprint
    // `footprint` cells wide, centred on the cell
    let foot = u.cell * max(u.footprint, 1.0);
    let centre = cell_min + 0.5 * u.cell;
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let q = centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot;
            sum += shown(vec2<i32>(floor(q)));
        }
    }
    let col = sum / 16.0;

    // Luminance, remapped: contrast about mid-grey, then brightness
    let lum = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    var t = clamp((lum - 0.5) * u.contrast + 0.5 + u.brightness, 0.0, 1.0);
    if u.levels >= 1.0 {
        t = round(t * u.levels) / u.levels;
    }
    let n = max(u.ramp_len, 1u);
    var step = min(u32(t * f32(n)), n - 1u);
    // Inverse: the ink fills the cell and the glyph is cut from it, so the
    // ramp runs backwards to keep bright cells bright
    let inverse = u.ground == 2u;
    if inverse {
        step = n - 1u - step;
    }

    var origin = ramp_origin(step);
    var coverage = glyph_coverage(origin, local);

    // On an edge, the line glyph the cell pass picked, over the tone glyph;
    // the glow follows whichever glyph mostly shows
    if u.edge_on == 1u {
        let slot = edge_slot(vec2<i32>(cell) + vec2<i32>(1));
        if slot < 4u {
            let pair = u.edge_glyphs[slot / 2u];
            let edge_origin = select(pair.xy, pair.zw, (slot & 1u) == 1u);
            coverage = mix(coverage, glyph_coverage(edge_origin, local), u.edges);
            if u.edges >= 0.5 {
                origin = edge_origin;
            }
        }
    }

    // Ink and ground
    let hue = col / max(max(col.r, max(col.g, col.b)), 1e-4);
    var ink = vec3<f32>(0.0);
    var ground = col * u.background;
    // Density carries brightness and the ink carries hue at full value, so
    // a dark object's sparse glyphs still read as its colour
    switch u.color_mode {
        case 1u: {
            ink = u.tint;
            ground = u.tint * lum * u.background;
        }
        case 2u: {
            ink = col;
        }
        case 3u: {
            ink = vec3<f32>(1.0);
            ground = vec3<f32>(lum * u.background);
        }
        case 4u: {
            ink = palette_ink(hue, t);
            ground = u.palette_bg;
        }
        default: {
            ink = hue;
        }
    }
    ink = clamp(ink * u.ink, vec3<f32>(0.0), vec3<f32>(1.0));
    if u.ground == 1u {
        ground = vec3<f32>(0.0);
    }
    if inverse {
        let fill = ink;
        ink = ground;
        ground = fill;
    }
    var out = mix(ground, ink, coverage);
    if u.glow > 0.0 && !inverse {
        out += ink * (u.glow * (0.35 + 0.65 * t)) * glyph_halo(origin, local, coverage);
    }
    out = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(select(out, to_linear(out), u.srgb == 1u), 1.0);
}
