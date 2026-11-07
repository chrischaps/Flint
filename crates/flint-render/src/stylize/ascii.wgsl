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
// Each per-entity variant cell's colour sum and sample count (#313,
// ascii_variant_cells.wgsl): the first two of its band's variants met in
// the cell, one texture each
@group(3) @binding(4) var variant_cells_a: texture_2d<u32>;
@group(3) @binding(5) var variant_cells_b: texture_2d<u32>;

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
// no ink. `glyph` is the font's glyph cell in atlas pixels (#335: each
// variant's own font).
fn glyph_coverage(origin: vec2<u32>, local: vec2<f32>, cell_size: vec2<f32>, glyph: vec2<f32>) -> f32 {
    let scale = cell_size / glyph;
    let gp = local / scale - 0.5;
    let base = floor(gp);
    let f = clamp((gp - base - 0.5) * scale + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    let o = vec2<i32>(origin);
    let hi = vec2<i32>(glyph);
    let b = vec2<i32>(base);
    let i0 = clamp(b, vec2<i32>(-1), hi);
    let i1 = clamp(b + vec2<i32>(1), vec2<i32>(-1), hi);
    let c00 = textureLoad(atlas, o + vec2<i32>(i0.x, i0.y), 0).r;
    let c10 = textureLoad(atlas, o + vec2<i32>(i1.x, i0.y), 0).r;
    let c01 = textureLoad(atlas, o + vec2<i32>(i0.x, i1.y), 0).r;
    let c11 = textureLoad(atlas, o + vec2<i32>(i1.x, i1.y), 0).r;
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

// The inks palette.wgsl's matcher reads
fn palette_ink(i: u32) -> vec3<f32> {
    return u.palette[i].rgb;
}

fn palette_ink_lab(i: u32) -> vec3<f32> {
    return u.palette_lab[i].rgb;
}

// The palette ink for a cell of full-value `hue` and tone `t` in look `v`,
// from its inks; `cell` places the cell in the dither pattern (#320)
fn cell_ink(v: Variant, hue: vec3<f32>, t: f32, cell: vec2<f32>) -> vec3<f32> {
    let n = max(v.palette_len, 1u);
    if v.palette_match == 1u {
        return u.palette[v.palette_start + min(u32(t * f32(n)), n - 1u)].rgb;
    }
    return pal_pick(v.palette_start, n, hue, v.chroma, v.palette_dither, vec2<i32>(cell));
}

// How much of the glyph lies within a couple of pixels of `local`, less
// what's under it: the halo a bright glyph throws on its own cell.
fn glyph_halo(origin: vec2<u32>, local: vec2<f32>, coverage: f32, cell_size: vec2<f32>, glyph: vec2<f32>) -> f32 {
    let r = max(cell_size.x / glyph.x, 1.0) * 1.25;
    var sum = 0.0;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * 0.7853982;
        let d = vec2<f32>(cos(a), sin(a));
        sum += glyph_coverage(origin, local + d * r, cell_size, glyph);
        sum += 0.5 * glyph_coverage(origin, local + d * r * 2.0, cell_size, glyph);
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
    v.chroma = u.chroma;
    v.palette_dither = u.palette_dither;
    v.glyph = u.glyph;
    v.edge_start = NO_EDGE_START;
    return v;
}

// A cell of colour `col`, tone `t` and ramp step `step`, drawn in look `v`
// at the pixel `local` in its cell; `edge` is the cell's line glyph slot
// (#267), or 4 for none. `floor` is the least ink the pixel takes (#288: a
// silhouette or a thin feature's pixel), 0 for the glyph alone.
// The tone glyph's atlas origin for ramp step `step_in` in look `v`.
// Inverse: the ink fills the cell and the glyph is cut from it, so the
// ramp runs backwards to keep bright cells bright.
fn tone_origin(v: Variant, step_in: u32) -> vec2<u32> {
    let n = max(v.ramp_len, 1u);
    var step = min(step_in, n - 1u);
    if v.ground == 2u {
        step = n - 1u - step;
    }
    return ramp_origin(v.ramp_start + step);
}

// The line glyph `edge`'s atlas origin in look `v`: the whole frame's, or
// (#335) the four a variant of another font keeps in the ramp table
fn edge_origin(v: Variant, edge: u32) -> vec2<u32> {
    if v.edge_start != NO_EDGE_START {
        return ramp_origin(v.edge_start + edge);
    }
    let pair = u.edge_glyphs[edge / 2u];
    return select(pair.xy, pair.zw, (edge & 1u) == 1u);
}

// The glyph a cell mostly shows (#317's text, and the glow's): the line
// glyph on an edge drawn at half strength or more, else the tone glyph.
fn shown_origin(v: Variant, step: u32, edge: u32) -> vec2<u32> {
    if edge < 4u && u.edges >= 0.5 {
        return edge_origin(v, edge);
    }
    return tone_origin(v, step);
}

// A cell's ink and ground colours, in look `v`, for colour `col` and tone
// `t` in cell `cell` (its place in the palette dither, #320); inverse has
// them swapped already.
struct Inks {
    ink: vec3<f32>,
    ground: vec3<f32>,
}

fn cell_inks(v: Variant, col: vec3<f32>, t: f32, cell: vec2<f32>) -> Inks {
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
            ink = cell_ink(v, hue, t, cell);
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
    var out: Inks;
    if v.ground == 2u {
        out.ink = ground;
        out.ground = ink;
    } else {
        out.ink = ink;
        out.ground = ground;
    }
    return out;
}

fn shade(v: Variant, col: vec3<f32>, t: f32, step: u32, cell: vec2<f32>, local: vec2<f32>, edge: u32, floor_in: f32) -> vec3<f32> {
    let inverse = v.ground == 2u;
    var coverage = glyph_coverage(tone_origin(v, step), local, v.cell, v.glyph);
    // On an edge, the line glyph over the tone glyph; the glow follows
    // whichever glyph mostly shows
    if edge < 4u {
        coverage = mix(coverage, glyph_coverage(edge_origin(v, edge), local, v.cell, v.glyph), u.edges);
    }
    let origin = shown_origin(v, step, edge);
    // Inverse already fills the cell with ink; a floor there would cut it
    if !inverse {
        coverage = max(coverage, floor_in);
    }
    let inks = cell_inks(v, col, t, cell);
    let ink = inks.ink;
    let ground = inks.ground;
    var out = mix(ground, ink, coverage);
    if v.glow > 0.0 && !inverse {
        out += ink * (v.glow * (0.35 + 0.65 * t)) * glyph_halo(origin, local, coverage, v.cell, v.glyph);
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

// What a per-entity variant's cell draws at a pixel: the cell's colour,
// tone, ramp step and line glyph slot, the pixel's place in the cell and
// the share of the cell's lattice samples that are the variant's.
struct Drawn {
    col: vec3<f32>,
    t: f32,
    step: u32,
    edge: u32,
    // The cell's index on the variant's grid
    cell: vec2<f32>,
    local: vec2<f32>,
    share: f32,
}

// A per-entity variant's cell at `p`: on its own grid, coloured by the
// lattice samples of its own variant only. The variant cell pass (#313)
// summed them once for the cell; a variant without a band, or a cell
// holding more of its band's variants than the pass keeps, sums here.
fn variant_cell(slot: u32, p: vec2<f32>) -> Drawn {
    let v = u.variants[slot];
    let rel = p - v.origin;
    let cell = floor(rel / v.cell);
    let cell_min = cell * v.cell + v.origin;
    let local = rel - cell * v.cell;
    var sum = vec3<f32>(0.0);
    var count = 0.0;
    var summed = false;
    if v.cell_row > 0u {
        let at = vec2<i32>(
            i32(cell.x) + CELL_OFFSET,
            i32(v.cell_row - 1u) + i32(cell.y) + CELL_OFFSET,
        );
        let first = textureLoad(variant_cells_a, at, 0);
        var entry = first;
        if (first.w & 255u) != slot {
            entry = textureLoad(variant_cells_b, at, 0);
        }
        if (entry.w & 255u) == slot {
            sum = bitcast<vec3<f32>>(entry.xyz);
            count = f32((entry.w >> 8u) & 255u);
            summed = true;
        } else if ((first.w >> 16u) & 1u) == 0u {
            // None of this variant's samples fell in the cell
            summed = true;
        }
    }
    if !summed {
        let foot = v.cell * max(v.footprint, 1.0);
        let centre = cell_min + 0.5 * v.cell;
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let q = floor(centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot);
                let w = select(0.0, 1.0, slot_at(q) == slot);
                sum += shown_entity(vec2<i32>(q)) * w;
                count += w;
            }
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
    var d: Drawn;
    d.col = col;
    d.t = t;
    d.step = step;
    d.edge = edge;
    d.cell = cell;
    d.local = local;
    d.share = count / 16.0;
    return d;
}

// A per-entity variant's pixel
fn variant_pixel(slot: u32, p: vec2<f32>) -> vec3<f32> {
    let v = u.variants[slot];
    let d = variant_cell(slot, p);
    // A feature thinner than a cell holds a sliver of one glyph, mostly
    // ground: at cell 8 a bench's 3 px leg is a dark smudge (#288). Two
    // floors keep it: the variant's own pixels along its silhouette take
    // full ink, so the mask's edge is drawn to the pixel, and a cell the
    // variant only grazes (under a quarter of its samples, easing out by
    // five eighths) inks its pixels at least `min_coverage`.
    var floor_ink = v.min_coverage * (1.0 - smoothstep(0.25, 0.625, d.share));
    if v.outline > 0.0 && on_silhouette(slot, p, v.outline) {
        floor_ink = 1.0;
    }
    return shade(v, d.col, d.t, d.step, d.cell, d.local, d.edge, floor_ink);
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
        out = shade(frame_look(), col, t, state.a, cell, local, edge_slot(cell), 0.0);
    }
    return vec4<f32>(select(out, to_linear(out), u.srgb == 1u), 1.0);
}

// The glyph grid as text (#317): one fragment a screen-grid cell, the
// same texel as the cell state, saying what fs_main draws at the centre of
// the cell's visible part. x = the glyph's atlas origin (x | y << 16), or
// TEXT_NONE where the picture passes through; y and z = the ink and the
// ground as 8-bit display RGB (the picture's colour in both when passed);
// w = the style mask id there, so the host can tell a cell a later style
// draws over. flint render --ascii-text reads it back.
const TEXT_NONE: u32 = 0xffffffffu;

fn pack_rgb(c: vec3<f32>) -> u32 {
    let b = vec3<u32>(round(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
    return b.r | (b.g << 8u) | (b.b << 16u);
}

fn text_passed(p: vec2<f32>) -> vec4<u32> {
    let c = pack_rgb(shown_entity(vec2<i32>(floor(p))));
    return vec4<u32>(TEXT_NONE, c, c, 0u);
}

fn text_cell(v: Variant, col: vec3<f32>, t: f32, step: u32, cell: vec2<f32>, edge: u32) -> vec4<u32> {
    let o = shown_origin(v, step, edge);
    let inks = cell_inks(v, col, t, cell);
    return vec4<u32>(o.x | (o.y << 16u), pack_rgb(inks.ink), pack_rgb(inks.ground), 0u);
}

// What fs_main draws at pixel `p` of the cell whose cell state texel is
// `index`, as a text texel (w left 0)
fn text_at(p: vec2<f32>, index: vec2<i32>, cell: vec2<f32>) -> vec4<u32> {
    var slot = 0u;
    var blend = 1.0;
    if u.variant_meta.x > 1u {
        let m = mask_texel(p);
        slot = slot_of(m.x);
        blend = f32(m.y) / 65535.0;
    }
    if slot != 0u {
        let v = u.variants[slot];
        if (v.flags & 1u) != 0u || blend <= 0.0 {
            return text_passed(p);
        }
        if blend < 1.0 && (v.flags & 2u) != 0u {
            if dissolve_order(floor((p - v.origin) / v.cell)) >= blend {
                return text_passed(p);
            }
        }
        let d = variant_cell(slot, p);
        return text_cell(v, d.col, d.t, d.step, d.cell, d.edge);
    }
    if u.variant_meta.y == 1u {
        return text_passed(p);
    }
    let state = textureLoad(cells, index, 0);
    let col = vec3<f32>(state.rgb) / UNORM16;
    let t = banded(tone(slot_luma(col, 0u)));
    return text_cell(frame_look(), col, t, state.a, cell, edge_slot(cell));
}

@fragment
fn fs_text(in: VertexOutput) -> @location(0) vec4<u32> {
    let index = vec2<i32>(floor(in.position.xy));
    let cell = vec2<f32>(index - vec2<i32>(CELL_OFFSET));
    // The centre pixel of the part of the cell on screen
    let lo = max(cell * u.cell + u.origin, vec2<f32>(0.0));
    let hi = min((cell + 1.0) * u.cell + u.origin, u.size);
    let p = floor((lo + hi) * 0.5) + 0.5;
    var out = text_at(p, index, cell);
    // The 1x1 stand-in reads 0 while no mask is bound
    out.w = mask_texel(p).x;
    return out;
}
