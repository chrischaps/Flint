// ASCII edges (#267): what the edge cells' readers share. Two passes
// decide each edge cell's glyph once per cell (#910): the thin pass
// (ascii_edge_thin.wgsl) packs each cell's thinned line and marked
// corner's arms with `pack_thin`, and the corner pass
// (ascii_edge_corners.wgsl) packs the glyph it draws with `pack_slots`.
// The glyph pass (ascii.wgsl) and the edge debug view
// (ascii_edge_view.wgsl, #349) read that one texel with `slot_for`. Each
// includer declares `edge_cells`, the edge cell pass's texture
// (ascii_edge_cells.wgsl: x = line glyph slot + 1 + half its direction in
// half turns, y = its length in cell widths, z = outline + 2 when the
// line's cross axis is x, w = the line's offset from the cell centre on
// that axis).
//
// An edge cell draws one of up to 13 glyphs, numbered as
// `EdgeGlyphs::slots()` packs them: 0..3 the lines - | / \, 4..7 the
// corners, 8..11 the tees, 12 the cross (#348); NO_EDGE for none.

const NO_EDGE: u32 = 15u;

fn edge_cell_at(c: vec2<i32>) -> vec4<f32> {
    let cmax = vec2<i32>(textureDimensions(edge_cells)) - vec2<i32>(1);
    return textureLoad(edge_cells, clamp(c, vec2<i32>(0), cmax), 0);
}

// The thin pass's texture is this many cells bigger than the edge cells
// on every side: the corner pass asks a cell's neighbours, one past the
// grid at its border, and the thinning there reads the clamped edge cells
// as it did when every pixel decided for itself
const THIN_PAD: i32 = 1;

// One thin pass texel (#910): the cell's thinned line slot (NO_EDGE for
// none) in bits 0..3, whether the cell pass marked it a corner in bit 4,
// and a marked cell's arms (right 1, left 2, down 4, up 8) in bits 5..8
fn pack_thin(line: u32, marked: bool, arms: u32) -> u32 {
    return line | (u32(marked) << 4u) | (arms << 5u);
}

fn thin_line(t: u32) -> u32 {
    return t & 15u;
}

fn thin_marked(t: u32) -> bool {
    return (t & 16u) != 0u;
}

fn thin_arms(t: u32) -> u32 {
    return (t >> 5u) & 15u;
}

// One edge slot texel (#910): the glyph the cell draws in a font with no
// corner glyphs (its thinned line) in bits 0..3, with the four corners in
// bits 4..7, with all nine in bits 8..11. A font has 0, 4 or 9
// (`EdgeGlyphs::corner_count`), and a cell's decision differs between
// them, so the pass makes all three and each pixel takes its font's.
// Above them (#477), what the corner pass found this frame for four and
// nine corners in bits 12..15 and 16..19, which can differ from what the
// cell draws while a new corner waits for a second frame to agree.
fn pack_slots(thin: u32, four: u32, nine: u32, found_four: u32, found_nine: u32) -> u32 {
    return thin | (four << 4u) | (nine << 8u) | (found_four << 12u) | (found_nine << 16u);
}

// The slot a font with `corners` corner glyphs draws, from a packed texel
fn slot_for(packed: u32, corners: u32) -> u32 {
    var shift = 0u;
    if corners >= 9u {
        shift = 8u;
    } else if corners > 0u {
        shift = 4u;
    }
    return (packed >> shift) & 15u;
}
