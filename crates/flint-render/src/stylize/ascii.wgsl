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
//
// Blend (#278): the mask's .g says how much of the style a pixel's entity
// asks for. Below 1 the glyph pixel is mixed with the picture as it came
// (fade), or the cell is drawn whole or not at all, by its patch noise
// (dissolve, flag 2).

@group(1) @binding(0) var cells: texture_2d<u32>;
@group(2) @binding(0) var atlas: texture_2d<f32>;
@group(3) @binding(0) var entity_input: texture_2d<f32>;
@group(3) @binding(1) var style_mask: texture_2d<u32>;
// The edge cell pass's texel per cell (#267, ascii_edge_cells.wgsl): x =
// line glyph slot + 1 (0 = none), y = its length in cell widths
@group(3) @binding(2) var edge_cells: texture_2d<f32>;
// Each slot's tone range (#289, ascii_stats_resolve.wgsl)
@group(3) @binding(3) var tone_stats: texture_2d<f32>;

// The tone luminance of a colour in `slot`: stretched over the slot's range
fn slot_luma(col: vec3<f32>, slot: u32) -> f32 {
    let amount = slot_normalize(slot);
    if amount <= 0.0 {
        return luma(col);
    }
    return stretched(luma(col), textureLoad(tone_stats, vec2<i32>(i32(slot), 0), 0), amount);
}

fn edge_cell_at(c: vec2<i32>) -> vec4<f32> {
    let cmax = vec2<i32>(textureDimensions(edge_cells)) - vec2<i32>(1);
    return textureLoad(edge_cells, clamp(c, vec2<i32>(0), cmax), 0);
}

// Whether neighbour `n` takes the line from a cell with `slot` and `len`:
// the same glyph, and more of the line (or as much, on the earlier side).
fn yields_to(n: vec4<f32>, slot: u32, len: f32, earlier: bool) -> bool {
    if n.x < 0.5 || min(u32(n.x - 0.5), 3u) != slot {
        return false;
    }
    return n.y > len || (n.y == len && earlier);
}

// The line glyph slot of the screen-grid cell `cell`, or 4 for none. A line
// running near a cell border splits between the two cells, and each would
// draw it: a cell holding less than one cell width of line yields to a
// neighbour across the line (left/right of `│`, above/below of `─`, either
// side of a diagonal) holding more. A full crossing always draws, so two
// real edges a cell apart both stay.
fn edge_slot(cell: vec2<f32>) -> u32 {
    if u.edge_on == 0u {
        return 4u;
    }
    let c = vec2<i32>(cell) + vec2<i32>(CELL_OFFSET);
    let e = edge_cell_at(c);
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
        if yields_to(edge_cell_at(c - off[k]), slot, len, true)
            || yields_to(edge_cell_at(c + off[k]), slot, len, false) {
            return 4u;
        }
    }
    return slot;
}

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
    v.exposure = u.exposure;
    v.stretch = u.stretch;
    return v;
}

// A cell of colour `col`, tone `t` and ramp step `step`, drawn in look `v`
// at the pixel `local` in its cell; `edge` is the cell's line glyph slot
// (#267), or 4 for none. `floor` is the least ink the pixel takes (#288: a
// silhouette or a thin feature's pixel), 0 for the glyph alone.
fn shade(v: Variant, col: vec3<f32>, t: f32, step_in: u32, local: vec2<f32>, edge: u32, floor_in: f32) -> vec3<f32> {
    let n = max(v.ramp_len, 1u);
    var step = min(step_in, n - 1u);
    // Inverse: the ink fills the cell and the glyph is cut from it, so the
    // ramp runs backwards to keep bright cells bright
    let inverse = v.ground == 2u;
    if inverse {
        step = n - 1u - step;
    }

    var origin = ramp_origin(v.ramp_start + step);
    var coverage = glyph_coverage(origin, local, v.cell);
    // On an edge, the line glyph over the tone glyph; the glow follows
    // whichever glyph mostly shows
    if edge < 4u {
        let pair = u.edge_glyphs[edge / 2u];
        let edge_origin = select(pair.xy, pair.zw, (edge & 1u) == 1u);
        coverage = mix(coverage, glyph_coverage(edge_origin, local, v.cell), u.edges);
        if u.edges >= 0.5 {
            origin = edge_origin;
        }
    }
    // Inverse already fills the cell with ink; a floor there would cut it
    if !inverse {
        coverage = max(coverage, floor_in);
    }
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
fn mask_texel(p: vec2<f32>) -> vec2<u32> {
    let msize = vec2<i32>(textureDimensions(style_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    return textureLoad(style_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).rg;
}

fn slot_of(id: u32) -> u32 {
    if id >= 256u {
        return 0u;
    }
    return u.slots[id / 4u][id % 4u];
}

fn slot_at(p: vec2<f32>) -> u32 {
    return slot_of(mask_texel(p).x);
}

fn cell_hash(c: vec2<f32>) -> f32 {
    return fract(sin(dot(c, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn value_noise(q: vec2<f32>) -> f32 {
    let i = floor(q);
    let f = fract(q);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(cell_hash(i), cell_hash(i + vec2<f32>(1.0, 0.0)), s.x),
        mix(cell_hash(i + vec2<f32>(0.0, 1.0)), cell_hash(i + vec2<f32>(1.0, 1.0)), s.x),
        s.y,
    );
}

// A cell's place in the dissolve order, 0..1: half patch noise a few cells
// across (so glyphs arrive in patches that grow, like a tear spreading),
// half per-cell hash (so a patch's edge is ragged and a small prop, which
// holds only a few noise lattice points, still turns about `blend` of its
// cells). The sum of two near-uniform values is triangular; its CDF maps
// it back to uniform, so the share of turned cells follows the blend.
fn dissolve_order(cell: vec2<f32>) -> f32 {
    let n = value_noise(cell * 0.3) * 0.65 + value_noise(cell * 0.6 + vec2<f32>(31.0, 7.0)) * 0.35;
    // Value noise piles up in the middle; spread it back toward 0..1
    let patches = clamp((n - 0.5) * 2.0 + 0.5, 0.0, 1.0);
    let s = 0.5 * (patches + cell_hash(cell + vec2<f32>(17.0, 5.0)));
    let u = select(1.0 - 2.0 * (1.0 - s) * (1.0 - s), 2.0 * s * s, s < 0.5);
    return clamp(u, 0.0, 0.9999);
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
    let raw = select(shown_entity(vec2<i32>(floor(p))), sum / max(count, 1.0), count > 0.0);
    let col = exposed(raw, exp2(v.exposure));
    var t = clamp((slot_luma(col, slot) - 0.5) * v.contrast + 0.5 + v.brightness, 0.0, 1.0);
    if v.levels >= 1.0 {
        t = round(t * v.levels) / v.levels;
    }
    let n = max(v.ramp_len, 1u);
    let step = min(u32(t * f32(n)), n - 1u);
    // The edge cells are on the screen grid: a variant on that grid shares
    // them; one on an object grid keeps its tone glyphs
    var edge = 4u;
    if all(v.cell == u.cell) && all(v.origin == u.origin) {
        edge = edge_slot(cell);
    }
    // A feature thinner than a cell holds a sliver of one glyph, mostly
    // ground: at cell 8 a bench's 3 px leg is a dark smudge (#288). Two
    // floors keep it: the variant's own pixels along its silhouette take
    // full ink, so the mask's edge is drawn to the pixel, and a cell the
    // variant only grazes (under a quarter of its samples, easing out by
    // five eighths) inks its pixels at least `min_coverage`.
    let share = count / 16.0;
    var floor_ink = v.min_coverage * (1.0 - smoothstep(0.25, 0.625, share));
    if v.outline > 0.0 && on_silhouette(slot, p, v.outline) {
        floor_ink = 1.0;
    }
    return shade(v, col, t, step, local, edge, floor_ink);
}

// Whether the pixel at `p`, of variant `slot`, lies within `width` output
// pixels of a pixel of another slot: rings of the 8 compass neighbours at
// each whole pixel out to the width (at most 4), the last at the width.
fn on_silhouette(slot: u32, p: vec2<f32>, width: f32) -> bool {
    let rings = min(i32(ceil(width)), 4);
    for (var k = 1; k <= rings; k++) {
        let r = min(f32(k), width);
        for (var i = 0; i < 8; i++) {
            let a = f32(i) * 0.7853982;
            // Diagonals step a whole pixel on each axis, so a 1 px
            // outline also sees the corner neighbours
            var d = vec2<f32>(cos(a), sin(a));
            if (i & 1) == 1 {
                d = sign(d);
            }
            if slot_at(p + d * r) != slot {
                return true;
            }
        }
    }
    return false;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    var slot = 0u;
    // How much of the style the pixel's entity asks for, 0..1 (mask .g)
    var blend = 1.0;
    if u.variant_meta.x > 1u {
        let m = mask_texel(p);
        slot = slot_of(m.x);
        blend = f32(m.y) / 65535.0;
    }
    var out: vec3<f32>;
    if slot != 0u {
        let v = u.variants[slot];
        if (v.flags & 1u) != 0u {
            return passed(p);
        }
        if blend <= 0.0 {
            return passed(p);
        }
        if blend < 1.0 && (v.flags & 2u) != 0u {
            let cell = floor((p - v.origin) / v.cell);
            if dissolve_order(cell) >= blend {
                return passed(p);
            }
        }
        out = variant_pixel(slot, p);
        if blend < 1.0 && (v.flags & 2u) == 0u {
            out = mix(shown_entity(vec2<i32>(floor(p))), out, blend);
        }
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
        let t = banded(tone(slot_luma(col, 0u)));
        out = shade(frame_look(), col, t, state.a, local, edge_slot(cell), 0.0);
    }
    return vec4<f32>(select(out, to_linear(out), u.srgb == 1u), 1.0);
}
