// ASCII edges (#267), the glyph pass's last say: which edge cells draw.
// Shared by the glyph pass (ascii.wgsl) and the edge debug view
// (ascii_edge_view.wgsl, #349), so the view shows the decision the glyphs
// make, not a copy of it. The includer declares `edge_cells`, the edge
// cell pass's texture (ascii_edge_cells.wgsl: x = line glyph slot + 1,
// y = its length in cell widths).

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

// The line glyph slot the edge cell at texel `c` draws, or 4 for none. A
// line running near a cell border splits between the two cells, and each
// would draw it: a cell holding less than one cell width of line yields to
// a neighbour across the line (left/right of `│`, above/below of `─`,
// either side of a diagonal) holding more. A full crossing always draws,
// so two real edges a cell apart both stay.
fn thinned_slot(c: vec2<i32>) -> u32 {
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
