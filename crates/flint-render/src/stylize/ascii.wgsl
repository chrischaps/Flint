// ASCII glyph pass (#265): draws the glyph the cell pass chose for each
// cell (ascii_cells.wgsl), inked in the cell's colour or a tint. Runs at
// the display stage, on the finished picture at output resolution, so the
// grid stays crisp under a render scale. ascii_common.wgsl comes first.

@group(1) @binding(0) var cells: texture_2d<u32>;
@group(2) @binding(0) var atlas: texture_2d<f32>;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let rel = p - u.origin;
    let cell = floor(rel / u.cell);
    let local = rel - cell * u.cell;

    // The cell's colour and glyph, as the cell pass left them
    let state = textureLoad(cells, vec2<i32>(cell) + vec2<i32>(CELL_OFFSET), 0);
    let col = vec3<f32>(state.rgb) / UNORM16;
    let lum = luma(col);
    let t = banded(tone(lum));
    let n = max(u.ramp_len, 1u);
    var step = min(state.a, n - 1u);
    // Inverse: the ink fills the cell and the glyph is cut from it, so the
    // ramp runs backwards to keep bright cells bright
    let inverse = u.ground == 2u;
    if inverse {
        step = n - 1u - step;
    }

    let origin = ramp_origin(step);
    let coverage = glyph_coverage(origin, local);

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
