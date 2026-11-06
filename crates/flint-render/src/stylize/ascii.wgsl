// ASCII glyph pass (#265): draws the glyph the cell pass chose for each
// cell (ascii_cells.wgsl), inked in the cell's colour or a tint. Runs at
// the display stage, on the finished picture at output resolution, so the
// grid stays crisp under a render scale. ascii_common.wgsl comes first.
//
// Per entity (#266): each pixel's style mask id picks a row of the variant
// table. Slot 0 is the whole frame's, drawn from the cell state; it passes
// the picture through when only entities asked for the style. Any other
// row is drawn here, per pixel, on its own grid: the cell's colour
// averages only the lattice samples of the pixel's own variant, so a
// styled prop's glyphs carry its colour, not the background's, and its
// silhouette is the mask's, to the pixel.

@group(1) @binding(0) var cells: texture_2d<u32>;
@group(2) @binding(0) var atlas: texture_2d<f32>;
@group(3) @binding(0) var entity_input: texture_2d<f32>;
@group(3) @binding(1) var style_mask: texture_2d<u32>;

fn ramp_origin(step: u32) -> vec2<u32> {
    let pair = u.ramp[step / 2u];
    return select(pair.xy, pair.zw, (step & 1u) == 1u);
}

// The glyph's coverage under the pixel at `local` (pixel centre, in the
// cell), from the glyph cell at `origin` in the atlas, for a cell
// `cell_size` pixels. Sharp bilinear: at a whole-number scale (8 px glyphs
// in 8, 16 or 24 px cells) it is exactly nearest; at a fractional one (12
// px, a 1.5x display) each atlas pixel keeps one width and only its border
// pixels blend, so strokes don't alternate 1 and 2 px. The atlas's
// one-pixel gutter is empty, so the neighbours past a glyph's edge read as
// no ink.
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

// The palette ink for a cell of full-value `hue` and tone `t`, from the
// inks at `start`
fn palette_ink(start: u32, len: u32, matching: u32, hue: vec3<f32>, t: f32) -> vec3<f32> {
    let n = max(len, 1u);
    if matching == 1u {
        return u.palette[start + min(u32(t * f32(n)), n - 1u)].rgb;
    }
    var best = u.palette[start].rgb;
    var best_d = 1e9;
    for (var i = 0u; i < n; i++) {
        let c = u.palette[start + i].rgb;
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
fn glyph_halo(origin: vec2<u32>, local: vec2<f32>, coverage: f32, cell_size: vec2<f32>) -> f32 {
    let r = max(cell_size.x / u.glyph.x, 1.0) * 1.25;
    var sum = 0.0;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * 0.7853982;
        let d = vec2<f32>(cos(a), sin(a));
        sum += glyph_coverage(origin, local + d * r, cell_size);
        sum += 0.5 * glyph_coverage(origin, local + d * r * 2.0, cell_size);
    }
    return max(sum / 12.0 - coverage, 0.0);
}

// The whole frame's settings as a variant row, so both paths ink alike
fn frame_look() -> Variant {
    var v: Variant;
    v.cell = u.cell;
    v.origin = u.origin;
    v.tint = u.tint;
    v.color_mode = u.color_mode;
    v.brightness = u.brightness;
    v.contrast = u.contrast;
    v.background = u.background;
    v.ink = u.ink;
    v.levels = u.levels;
    v.footprint = u.footprint;
    v.ramp_start = 0u;
    v.ramp_len = u.ramp_len;
    v.flags = 0u;
    v.glow = u.glow;
    v.ground = u.ground;
    v.palette_start = 0u;
    v.palette_bg = u.palette_bg;
    v.palette_len = u.palette_len;
    v.palette_match = u.palette_match;
    return v;
}

// A cell of colour `col`, tone `t` and ramp step `step`, drawn in look `v`
// at the pixel `local` in its cell
fn shade(v: Variant, col: vec3<f32>, t: f32, step_in: u32, local: vec2<f32>) -> vec3<f32> {
    let n = max(v.ramp_len, 1u);
    var step = min(step_in, n - 1u);
    // Inverse: the ink fills the cell and the glyph is cut from it, so the
    // ramp runs backwards to keep bright cells bright
    let inverse = v.ground == 2u;
    if inverse {
        step = n - 1u - step;
    }

    let origin = ramp_origin(v.ramp_start + step);
    let coverage = glyph_coverage(origin, local, v.cell);
    let lum = luma(col);

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
        case 3u: {
            ink = vec3<f32>(1.0);
            ground = vec3<f32>(lum * v.background);
        }
        case 4u: {
            ink = palette_ink(v.palette_start, v.palette_len, v.palette_match, hue, t);
            ground = v.palette_bg;
        }
        default: {
            ink = hue;
        }
    }
    ink = clamp(ink * v.ink, vec3<f32>(0.0), vec3<f32>(1.0));
    if v.ground == 1u {
        ground = vec3<f32>(0.0);
    }
    if inverse {
        let fill = ink;
        ink = ground;
        ground = fill;
    }
    var out = mix(ground, ink, coverage);
    if v.glow > 0.0 && !inverse {
        out += ink * (v.glow * (0.35 + 0.65 * t)) * glyph_halo(origin, local, coverage, v.cell);
    }
    return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
}

// The variant slot of the pixel at `p` (output pixels): the mask is at the
// scene's resolution, so it is read by position scaled to it.
fn slot_at(p: vec2<f32>) -> u32 {
    let msize = vec2<i32>(textureDimensions(style_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    let id = textureLoad(style_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r;
    if id >= 256u {
        return 0u;
    }
    return u.slots[id / 4u][id % 4u];
}

fn shown_entity(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let c = clamp(textureLoad(entity_input, clamp(p, vec2<i32>(0), max_px), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

// The picture as it came, for pixels no style of this one's covers
fn passed(p: vec2<f32>) -> vec4<f32> {
    return textureLoad(entity_input, vec2<i32>(floor(p)), 0);
}

// A per-entity variant's pixel: its cell on its own grid, coloured by the
// lattice samples of its own variant only
fn variant_pixel(slot: u32, p: vec2<f32>) -> vec3<f32> {
    let v = u.variants[slot];
    let rel = p - v.origin;
    let cell = floor(rel / v.cell);
    let cell_min = cell * v.cell + v.origin;
    let local = rel - cell * v.cell;
    let foot = v.cell * max(v.footprint, 1.0);
    let centre = cell_min + 0.5 * v.cell;
    var sum = vec3<f32>(0.0);
    var count = 0.0;
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let q = floor(centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot);
            let w = select(0.0, 1.0, slot_at(q) == slot);
            sum += shown_entity(vec2<i32>(q)) * w;
            count += w;
        }
    }
    let col = select(shown_entity(vec2<i32>(floor(p))), sum / max(count, 1.0), count > 0.0);
    var t = clamp((luma(col) - 0.5) * v.contrast + 0.5 + v.brightness, 0.0, 1.0);
    if v.levels >= 1.0 {
        t = round(t * v.levels) / v.levels;
    }
    let n = max(v.ramp_len, 1u);
    let step = min(u32(t * f32(n)), n - 1u);
    return shade(v, col, t, step, local);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    var slot = 0u;
    if u.variant_meta.x > 1u {
        slot = slot_at(p);
    }
    var out: vec3<f32>;
    if slot != 0u {
        if (u.variants[slot].flags & 1u) != 0u {
            return passed(p);
        }
        out = variant_pixel(slot, p);
    } else {
        if u.variant_meta.y == 1u {
            return passed(p);
        }
        let rel = p - u.origin;
        let cell = floor(rel / u.cell);
        let local = rel - cell * u.cell;

        // The cell's colour and glyph, as the cell pass left them
        let state = textureLoad(cells, vec2<i32>(cell) + vec2<i32>(CELL_OFFSET), 0);
        let col = vec3<f32>(state.rgb) / UNORM16;
        let t = banded(tone(luma(col)));
        out = shade(frame_look(), col, t, state.a, local);
    }
    return vec4<f32>(select(out, to_linear(out), u.srgb == 1u), 1.0);
}
