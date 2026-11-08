// ASCII edges (#267), the glyph pass's last say: which edge cells draw.
// Shared by the glyph pass (ascii.wgsl) and the edge debug view
// (ascii_edge_view.wgsl, #349), so the view shows the decision the glyphs
// make, not a copy of it. The includer declares `edge_cells`, the edge
// cell pass's texture (ascii_edge_cells.wgsl: x = line glyph slot + 1 +
// half its direction in half turns, y = its length in cell widths, z =
// outline + 2 when the line's cross axis is x, w = the line's offset from
// the cell centre on that axis).

fn edge_cell_at(c: vec2<i32>) -> vec4<f32> {
    let cmax = vec2<i32>(textureDimensions(edge_cells)) - vec2<i32>(1);
    return textureLoad(edge_cells, clamp(c, vec2<i32>(0), cmax), 0);
}

// Two cells' lines run the same way if they are less than this many half
// turns apart (10 degrees)
const SAME_TURN: f32 = 10.0 / 180.0;

// The direction of an edge cell's line, as a fraction of a half turn: the
// cell pass adds half of it to x, past the slot (or below -0.25, for a
// cell with edge pixels and no glyph)
fn edge_turn(e: vec4<f32>) -> f32 {
    if e.x > 0.5 {
        return 2.0 * (e.x - 1.0 - floor(e.x - 0.5));
    }
    return 2.0 * (-e.x - 0.25);
}

// The line glyph slot the edge cell at texel `c` draws, or 4 for none.
//
// A line belongs to the cell it passes within half a cell of, measured
// across it: down the centre column for a line nearer horizontal, along
// the centre row for one nearer vertical (#352). That is the cell a
// Bresenham line would light, so a straight line draws one chain of
// cells, at any angle, and two real edges a cell apart each keep their
// own. Comparing lengths with a neighbour (#267's rule) left two chains on
// coarse diagonals and at render scale 0.5, where both cells beside a
// line hold a full cell width of it.
//
// A line wider than a pixel can lie across the border, its centroid in
// each cell inside that cell: the cell takes the neighbour on the side it
// leans to into account, if that neighbour sees the same line there, and
// judges the pair's centroid, which the neighbour judges too, so exactly
// one of them draws.
fn thinned_slot(c: vec2<i32>) -> u32 {
    let e = edge_cell_at(c);
    if e.x < 0.5 {
        return 4u;
    }
    let slot = min(u32(e.x - 0.5), 3u);
    let cross_x = e.z > 1.5;
    var offset = e.w;
    let side = select(-1, 1, offset >= 0.0);
    let step = select(vec2<i32>(0, side), vec2<i32>(side, 0), cross_x);
    let n = edge_cell_at(c + step);
    // The neighbour's offset, from this cell's centre
    let theirs = n.w + f32(side);
    // The same line if it runs the same way (within 10 degrees: a box's
    // silhouette and the crease below it converge at its corner, a band's
    // halves run side by side) and their centroids are nearer than three
    // quarters of a cell: a wide band (a silhouette at render scale 0.5)
    // splits into halves up to half a cell apart, two real edges are a
    // cell apart
    var turn_apart = abs(edge_turn(e) - edge_turn(n));
    turn_apart = min(turn_apart, 1.0 - turn_apart);
    if n.y > 0.0 && (n.z > 1.5) == cross_x && turn_apart < SAME_TURN
        && abs(theirs - offset) < 0.75 {
        offset = (e.y * offset + n.y * theirs) / (e.y + n.y);
    }
    // Half open, so a line exactly on the border is one cell's. A line
    // past the border stays here if the cell it leans into draws no line
    // (it holds a sliver, under the threshold): the outline keeps no gap
    if (offset > -0.5 && offset <= 0.5) || n.x < 0.5 {
        return slot;
    }
    return 4u;
}
