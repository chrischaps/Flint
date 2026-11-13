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
// (fade), or the cell is drawn whole or not at all, by its rank in the
// patch noise among its prop's cells (dissolve, flag 2; #398).
//
// Cell silhouettes (#338): a variant with flag 4 (`silhouette = "cell"`)
// draws a cell whole or not at all. A cell of its grid is its when at least
// `cell_share` of the cell's 16 lattice samples are its pixels; every pixel
// of such a cell draws as the variant, whatever the mask says there, and
// its own pixels in a cell it doesn't hold draw as the whole frame does. So
// each pixel asks, per grid a snapping variant uses, which variant holds its
// cell (`resolve`): the question is the cell's, and every pixel of the cell
// gets the same answer.

@group(1) @binding(0) var cells: texture_2d<u32>;
@group(2) @binding(0) var atlas: texture_2d<f32>;
@group(3) @binding(0) var entity_input: texture_2d<f32>;
@group(3) @binding(1) var style_mask: texture_2d<u32>;
// The edge cell pass's texel per cell (#267, ascii_edge_cells.wgsl): x =
// line glyph slot + 1 (0 = none), y = its length in cell widths, z = the
// outline and cross-axis bits plus the near side's hue above them, 7 bits
// a channel (#351), w = the line's offset (#352)
@group(3) @binding(2) var edge_cells: texture_2d<f32>;
// Each slot's tone range (#289, ascii_stats_resolve.wgsl)
@group(3) @binding(3) var tone_stats: texture_2d<f32>;
// Each per-entity variant cell's colour sum and sample count (#313,
// ascii_variant_cells.wgsl): the first two of its band's variants met in
// the cell, one texture each
@group(3) @binding(4) var variant_cells_a: texture_2d<u32>;
@group(3) @binding(5) var variant_cells_b: texture_2d<u32>;
// Each dissolving variant's cells' ranks in its dissolve order (#398,
// ascii_dissolve_rank.wgsl)
@group(3) @binding(6) var dissolve_ranks: texture_2d<f32>;
// Each edge cell's glyph, decided once per cell (#910,
// ascii_edge_corners.wgsl): the slots packed for 0, 4 and 9 corner glyphs
@group(3) @binding(7) var edge_slots: texture_2d<u32>;

// The tone luminance of a colour in `slot`: stretched over the slot's range
fn slot_luma(col: vec3<f32>, slot: u32) -> f32 {
    let amount = slot_normalize(slot);
    if amount <= 0.0 {
        return luma(col);
    }
    return stretched(luma(col), textureLoad(tone_stats, vec2<i32>(i32(slot), 0), 0), amount);
}

// The edge glyph slot of the screen-grid cell `cell`, or NO_EDGE: the
// edge cell's line, thinned where a line splits between two cells, or the
// corner its neighbours' lines meet in (#348) when the drawing font has
// `corners` corner glyphs. The edge thin and corner passes decided it for
// the cell (#910, ascii_edge_corners.wgsl); ascii_edge_slot.wgsl follows
// this file.
fn edge_slot(cell: vec2<f32>, corners: u32) -> u32 {
    if u.edge_on == 0u {
        return NO_EDGE;
    }
    let cmax = vec2<i32>(textureDimensions(edge_slots)) - vec2<i32>(1);
    let c = clamp(vec2<i32>(cell) + vec2<i32>(CELL_OFFSET), vec2<i32>(0), cmax);
    return slot_for(textureLoad(edge_slots, c, 0).x, corners);
}

// How many corner glyphs look `v`'s font has (#348): a variant in a font
// of its own keeps its count, one in the whole frame's reads the frame's
fn look_corners(v: Variant) -> u32 {
    return select(v.corners, u.corners, v.edge_start == NO_EDGE_START);
}

// The near side's hue along screen-grid cell `cell`'s edge (#351), at full
// value: the object's own colour where the cell's is a mix of it and what's
// behind
fn edge_hue(cell: vec2<f32>) -> vec3<f32> {
    // Exact: w is a whole number under 2^24 (no +0.5, which f32 spacing
    // of 1 above 2^23 rounds up, carrying red into green)
    let w = u32(edge_cell_at(vec2<i32>(cell) + vec2<i32>(CELL_OFFSET)).z) >> 2u;
    // (& 127 on blue too: bit 23 above it is the corner mark, #348)
    return vec3<f32>(f32(w & 127u), f32((w >> 7u) & 127u), f32((w >> 14u) & 127u)) / 127.0;
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
// at the pixel `local` in its cell; `edge` is the cell's edge glyph slot
// (#267's lines, #348's corners), or NO_EDGE for none. `floor` is the
// least ink the pixel takes (#288: a silhouette or a thin feature's
// pixel), 0 for the glyph alone.
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
    if edge != NO_EDGE && u.edges >= 0.5 {
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

// The glyph ink look `v`'s colour mode gives colour `col` at tone `t`, in
// cell `cell`. Density carries brightness and the ink carries hue at full
// value, so a dark object's sparse glyphs still read as its colour.
fn mode_ink(v: Variant, col: vec3<f32>, t: f32, cell: vec2<f32>) -> vec3<f32> {
    let hue = col / max(max(col.r, max(col.g, col.b)), 1e-4);
    var ink = hue;
    switch v.color_mode {
        case 1u: {
            ink = v.tint;
        }
        case 2u: {
            ink = col;
        }
        case 3u: {
            ink = vec3<f32>(1.0);
        }
        case 4u: {
            ink = cell_ink(v, hue, t, cell);
        }
        default: {}
    }
    return clamp(ink * v.ink, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn cell_inks(v: Variant, col: vec3<f32>, t: f32, cell: vec2<f32>) -> Inks {
    let lum = luma(col);
    let ink = mode_ink(v, col, t, cell);
    var ground = col * v.background;
    switch v.color_mode {
        case 1u: {
            ground = v.tint * lum * v.background;
        }
        case 3u: {
            ground = vec3<f32>(lum * v.background);
        }
        case 4u: {
            ground = v.palette_bg;
        }
        default: {}
    }
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

// Whether cell `edge`'s line glyph takes an ink of its own (#351)
fn own_edge_ink(edge: u32) -> bool {
    return edge != NO_EDGE && u.edge_ink != 0u;
}

// The line glyph's own ink (#351) in look `v`, for the cell `cell` of colour
// `col` and tone `t`: the near side's hue at the cell's value, inked as the
// colour mode inks a cell (hue); or `edge_color` mixed toward that by
// `edge_tint` (color). `cell` is on the screen grid, as the edges are.
fn edge_ink(v: Variant, col: vec3<f32>, t: f32, cell: vec2<f32>) -> vec3<f32> {
    let near = edge_hue(cell) * max(max(col.r, max(col.g, col.b)), 1e-4);
    return line_ink(v, near, t, cell);
}

// The edge ink for a line whose near side is colour `near`
fn line_ink(v: Variant, near: vec3<f32>, t: f32, cell: vec2<f32>) -> vec3<f32> {
    let own = mode_ink(v, near, t, cell);
    if u.edge_ink == 1u {
        return mix(u.edge_color.rgb, own, u.edge_tint);
    }
    return own;
}

fn shade(v: Variant, col: vec3<f32>, t: f32, step: u32, cell: vec2<f32>, local: vec2<f32>, edge: u32, floor_in: f32) -> vec3<f32> {
    let inverse = v.ground == 2u;
    if own_edge_ink(edge) {
        return shade_edge(v, col, t, step, cell, local, edge, floor_in);
    }
    var coverage = glyph_coverage(tone_origin(v, step), local, v.cell, v.glyph);
    // On an edge, the line glyph over the tone glyph; the glow follows
    // whichever glyph mostly shows
    if edge != NO_EDGE {
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

// An edge cell whose line glyph has its own ink (#351): the tone glyph
// fades out under the line as `edges` rises (as in `shade`), in the cell's
// ink, and the line glyph is laid over it in the edge ink, on either
// ground (inverse: over the ink-filled cell).
fn shade_edge(v: Variant, col: vec3<f32>, t: f32, step: u32, cell: vec2<f32>, local: vec2<f32>, edge: u32, floor_in: f32) -> vec3<f32> {
    let inverse = v.ground == 2u;
    var tone_c = glyph_coverage(tone_origin(v, step), local, v.cell, v.glyph) * (1.0 - u.edges);
    if !inverse {
        tone_c = max(tone_c, floor_in);
    }
    let line_c = glyph_coverage(edge_origin(v, edge), local, v.cell, v.glyph) * u.edges;
    let inks = cell_inks(v, col, t, cell);
    let line_ink = edge_ink(v, col, t, cell);
    var out = mix(mix(inks.ground, inks.ink, tone_c), line_ink, line_c);
    if v.glow > 0.0 && !inverse {
        let line_shows = u.edges >= 0.5;
        let glow_ink = select(inks.ink, line_ink, line_shows);
        out += glow_ink * (v.glow * (0.35 + 0.65 * t))
            * glyph_halo(shown_origin(v, step, edge), local, max(tone_c, line_c), v.cell, v.glyph);
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

fn slot_at(p: vec2<f32>) -> u32 {
    return slot_of(mask_texel(p).x);
}

// A variant's flag: its silhouette snaps to whole cells (#338)
const SNAP: u32 = 4u;

// Whether slot `slot` is a drawn variant whose silhouette snaps
fn snaps(slot: u32) -> bool {
    return slot != 0u && (variants[slot].flags & (SNAP | 1u)) == SNAP;
}

// Who draws a pixel: a variant slot (0 = the whole frame), how much of
// the style it asks for (the mask's .g), and where the picture is read
// when it passes through (the pixel itself, but see `scene_behind`)
struct Owner {
    slot: u32,
    blend: f32,
    read_at: vec2<f32>,
}

// The lattice sample `k` (0..16, row by row) of the cell whose top left is
// `cell_min`, `cell` wide: a pixel's top left
fn lattice(cell_min: vec2<f32>, cell: vec2<f32>, k: i32) -> vec2<f32> {
    return floor(cell_min + (vec2<f32>(f32(k & 3), f32(k >> 2u)) + 0.5) / 4.0 * cell);
}

// A snapped variant's pixel in a cell it doesn't hold shows the scene, not
// its own smooth shading (#338): the picture at the nearest of the cell's
// lattice samples that isn't the variant's, so a dropped edge cell reads as
// what's around the entity, a quarter cell at a time.
fn scene_behind(p: vec2<f32>, slot: u32) -> vec2<f32> {
    let v = variants[slot];
    let c = floor((p - v.origin) / v.cell);
    let cell_min = c * v.cell + v.origin;
    var best = p;
    var best_d = 1e9;
    for (var k = 0; k < 16; k++) {
        let q = lattice(cell_min, v.cell, k);
        let d = dot(q + 0.5 - p, q + 0.5 - p);
        if d < best_d && slot_at(q) != slot {
            best_d = d;
            best = q + 0.5;
        }
    }
    return best;
}

fn same_grid(slot: u32, cell: vec2<f32>, origin: vec2<f32>) -> bool {
    let v = variants[slot];
    return all(v.cell == cell) && all(v.origin == origin);
}

// The snapping variant on the grid (`cell`, `origin`) that holds the cell
// under `p`: of the variants on that grid that snap, the one with the most
// of the cell's 16 lattice samples (the first met on a tie), if it has at
// least its `cell_share` of them; slot 0 for none. One pass, in registers:
// it counts the first two such variants met in the cell, which is every one
// unless three snapping variants of one grid meet in a cell (the third is
// then left out). Private arrays would spill to memory: every pixel on the
// screen runs this once a snapping grid. The blend is the one at the
// variant's first sample, so the whole cell fades alike.
fn grid_owner(p: vec2<f32>, cell: vec2<f32>, origin: vec2<f32>) -> Owner {
    let c = floor((p - origin) / cell);
    let cell_min = c * cell + origin;
    var a = 0u;
    var a_n = 0u;
    var a_blend = 1.0;
    var b = 0u;
    var b_n = 0u;
    var b_blend = 1.0;
    for (var k = 0; k < 16; k++) {
        let m = mask_texel(lattice(cell_min, cell, k));
        // Most samples are unstyled: no table reads for them. No
        // `continue`: FXC takes a second longer over each copy of a loop
        // that has one (#910)
        var s = 0u;
        if m.x != 0u {
            s = slot_of(m.x);
        }
        if s == 0u {
        } else if s == a {
            a_n += 1u;
        } else if s == b {
            b_n += 1u;
        } else if snaps(s) && same_grid(s, cell, origin) {
            if a == 0u {
                a = s;
                a_n = 1u;
                a_blend = f32(m.y) / 65535.0;
            } else if b == 0u {
                b = s;
                b_n = 1u;
                b_blend = f32(m.y) / 65535.0;
            }
        }
    }
    var out: Owner;
    out.slot = 0u;
    out.blend = 1.0;
    out.read_at = p;
    if b_n > a_n {
        a = b;
        a_n = b_n;
        a_blend = b_blend;
    }
    if a != 0u && f32(a_n) + 1e-3 >= variants[a].cell_share * 16.0 {
        out.slot = a;
        out.blend = a_blend;
    }
    return out;
}

// Who draws the pixel at `p`: the mask's slot, or with snapping variants
// in the frame (#338) the variant holding the pixel's cell. A pixel of a
// variant that cuts its silhouette to the pixel keeps its own; the pixel's
// own variant's grid is asked first, then each snapping grid in turn.
// `grid_owner` is called from one place, in one loop (g = -1 is the own
// grid): FXC inlines every call, and each copy cost the glyph shader
// seconds to compile on DX12 (#910).
fn resolve(p: vec2<f32>) -> Owner {
    var o: Owner;
    o.slot = 0u;
    o.blend = 1.0;
    o.read_at = p;
    if u.variant_meta.x <= 1u {
        return o;
    }
    let m = mask_texel(p);
    o.slot = slot_of(m.x);
    o.blend = f32(m.y) / 65535.0;
    if u.variant_meta.w == 0u || (o.slot != 0u && !snaps(o.slot)) {
        return o;
    }
    var own_cell = vec2<f32>(-1.0);
    var own_origin = vec2<f32>(0.0);
    if o.slot != 0u {
        own_cell = variants[o.slot].cell;
        own_origin = variants[o.slot].origin;
    }
    let grids = i32(min(u.variant_meta.w, MAX_SNAP_GRIDS));
    for (var g = select(0, -1, o.slot != 0u); g < grids; g++) {
        var cell = own_cell;
        var origin = own_origin;
        if g >= 0 {
            let grid = u.snap_grids[g];
            cell = grid.xy;
            origin = grid.zw;
        }
        // The own grid is asked once, first
        if g < 0 || !(all(cell == own_cell) && all(origin == own_origin)) {
            let w = grid_owner(p, cell, origin);
            if w.slot != 0u {
                return w;
            }
        }
    }
    // Nobody holds the cell: the whole frame's, and a dropped pixel of a
    // snapped variant shows the scene around it
    if o.slot != 0u {
        o.read_at = scene_behind(p, o.slot);
    }
    o.slot = 0u;
    o.blend = 1.0;
    return o;
}

// Where a pixel's cell stands in its variant's dissolve (#398): the cell's
// rank among the variant's cells nearby (ascii_dissolve_rank.wgsl), so a
// prop turns `blend` of its cells however few it has. A cell the rank
// pass didn't see as the variant's (a sliver between lattice samples), or
// a variant past MAX_DISSOLVE, takes its place in the order itself.
fn dissolve_place(slot: u32, p: vec2<f32>) -> f32 {
    let v = variants[slot];
    let cell = floor((p - v.origin) / v.cell);
    if v.dissolve_row > 0u {
        let at = vec2<i32>(cell) + vec2<i32>(CELL_OFFSET, i32(v.dissolve_row - 1u) + CELL_OFFSET);
        let size = vec2<i32>(textureDimensions(dissolve_ranks));
        if all(at >= vec2<i32>(0)) && all(at < size) {
            let r = textureLoad(dissolve_ranks, at, 0).r;
            if r >= 0.0 {
                return r;
            }
        }
    }
    return dissolve_order(cell);
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
    let v = variants[slot];
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
    let step = ramp_step(t, v.ramp_start, n);
    // The edge cells are on the screen grid: a variant on that grid shares
    // them; one on an object grid keeps its tone glyphs
    var edge = NO_EDGE;
    if all(v.cell == u.cell) && all(v.origin == u.origin) {
        edge = edge_slot(cell, look_corners(v));
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
    let v = variants[slot];
    let d = variant_cell(slot, p);
    // A feature thinner than a cell holds a sliver of one glyph, mostly
    // ground: at cell 8 a bench's 3 px leg is a dark smudge (#288). Two
    // floors keep it: the variant's own pixels along its silhouette take
    // full ink, so the mask's edge is drawn to the pixel, and a cell the
    // variant only grazes (under a quarter of its samples, easing out by
    // five eighths) inks its pixels at least `min_coverage`.
    var floor_ink = v.min_coverage * (1.0 - smoothstep(0.25, 0.625, d.share));
    // A snapped silhouette (#338) is the cells' own edge: no pixel outline
    if v.outline > 0.0 && (v.flags & SNAP) == 0u && on_silhouette(slot, p, v.outline) {
        // The silhouette is an outline too (#351): with an edge ink of its
        // own it takes that, the pixel being the near side itself
        if u.edge_ink != 0u && v.ground != 2u {
            return line_ink(v, d.col, d.t, d.cell);
        }
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
    // The whole frame's cell, and its state as the cell pass left it, read
    // before the mask is: the two loads are in flight together rather
    // than one after the other, which is what a whole-frame pixel paid for
    // per-entity variants being on screen (#337)
    let rel = p - u.origin;
    let cell = floor(rel / u.cell);
    let state = textureLoad(cells, vec2<i32>(cell) + vec2<i32>(CELL_OFFSET), 0);
    // Who draws the pixel, and how much of the style its entity asks for,
    // 0..1 (mask .g)
    let owner = resolve(p);
    let slot = owner.slot;
    let blend = owner.blend;
    var out: vec3<f32>;
    if slot != 0u {
        let v = variants[slot];
        if (v.flags & 1u) != 0u {
            return passed(p);
        }
        if blend <= 0.0 {
            return passed(p);
        }
        if blend < 1.0 && (v.flags & 2u) != 0u {
            if dissolve_place(slot, p) >= blend {
                return passed(p);
            }
        }
        out = variant_pixel(slot, p);
        if blend < 1.0 && (v.flags & 2u) == 0u {
            out = mix(shown_entity(vec2<i32>(floor(p))), out, blend);
        }
    } else {
        if u.variant_meta.y == 1u {
            return passed(owner.read_at);
        }
        let local = rel - cell * u.cell;
        let col = vec3<f32>(state.rgb) / UNORM16;
        let t = banded(tone(slot_luma(col, 0u)));
        out = shade(frame_look(), col, t, state.a, cell, local, edge_slot(cell, u.corners), 0.0);
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
    var ink = inks.ink;
    // The line glyph shows, in its own ink (#351)
    if own_edge_ink(edge) && u.edges >= 0.5 {
        ink = edge_ink(v, col, t, cell);
    }
    return vec4<u32>(o.x | (o.y << 16u), pack_rgb(ink), pack_rgb(inks.ground), 0u);
}

// What fs_main draws at pixel `p` of the cell whose cell state texel is
// `index`, as a text texel (w left 0)
fn text_at(p: vec2<f32>, index: vec2<i32>, cell: vec2<f32>) -> vec4<u32> {
    let owner = resolve(p);
    let slot = owner.slot;
    let blend = owner.blend;
    if slot != 0u {
        let v = variants[slot];
        if (v.flags & 1u) != 0u || blend <= 0.0 {
            return text_passed(p);
        }
        if blend < 1.0 && (v.flags & 2u) != 0u {
            if dissolve_place(slot, p) >= blend {
                return text_passed(p);
            }
        }
        let d = variant_cell(slot, p);
        return text_cell(v, d.col, d.t, d.step, d.cell, d.edge);
    }
    if u.variant_meta.y == 1u {
        return text_passed(owner.read_at);
    }
    let state = textureLoad(cells, index, 0);
    let col = vec3<f32>(state.rgb) / UNORM16;
    let t = banded(tone(slot_luma(col, 0u)));
    return text_cell(frame_look(), col, t, state.a, cell, edge_slot(cell, u.corners));
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
