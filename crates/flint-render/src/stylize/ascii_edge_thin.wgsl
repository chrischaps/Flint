// ASCII edges (#267), the glyph pass's last say: which edge cells draw.
// Shared by the glyph pass (ascii.wgsl) and the edge debug view
// (ascii_edge_view.wgsl, #349), so the view shows the decision the glyphs
// make, not a copy of it. The includer declares `edge_cells`, the edge
// cell pass's texture (ascii_edge_cells.wgsl: x = line glyph slot + 1 +
// half its direction in half turns, y = its length in cell widths, z =
// outline + 2 when the line's cross axis is x, w = the line's offset from
// the cell centre on that axis).
//
// An edge cell draws one of up to 13 glyphs, numbered as
// `EdgeGlyphs::slots()` packs them: 0..3 the lines - | / \, 4..7 the
// corners, 8..11 the tees, 12 the cross (#348); NO_EDGE for none.

const NO_EDGE: u32 = 15u;

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
// Whether an edge cell's line runs nearer vertical (its cross axis is x):
// bit 1 of z, which also carries the outline bit and the near side's hue
// above them (#351)
fn edge_crosses_x(e: vec4<f32>) -> bool {
    return (u32(e.z) & 2u) != 0u;
}

fn thinned_slot(c: vec2<i32>) -> u32 {
    let e = edge_cell_at(c);
    if e.x < 0.5 {
        return NO_EDGE;
    }
    let slot = min(u32(e.x - 0.5), 3u);
    let cross_x = edge_crosses_x(e);
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
    if n.y > 0.0 && edge_crosses_x(n) == cross_x && turn_apart < SAME_TURN
        && abs(theirs - offset) < 0.75 {
        offset = (e.y * offset + n.y * theirs) / (e.y + n.y);
    }
    // Half open, so a line exactly on the border is one cell's. A line
    // past the border stays here if the cell it leans into draws no line
    // (it holds a sliver, under the threshold): the outline keeps no gap
    if (offset > -0.5 && offset <= 0.5) || n.x < 0.5 {
        return slot;
    }
    return NO_EDGE;
}

// The edge cell pass's corner mark in z (#348, ascii_edge_cells.wgsl):
// the cell's edge pixels run in no one direction
const CORNER_BIT: u32 = 0x800000u;

// The glyph a cell's arms make, by arms right 1, left 2, down 4, up 8: a
// corner, tee or cross (4..12), the straight line for two opposite arms,
// or NO_EDGE (keep the cell's own line). A table, not a `switch`, which
// FXC has miscompiled (CLAUDE.md).
const ARM_GLYPHS = array<u32, 16>(
    NO_EDGE, NO_EDGE, NO_EDGE, 0u, // -, right, left, right+left
    NO_EDGE, 4u, 5u, 10u,          // down, right+down, left+down, right+left+down
    NO_EDGE, 6u, 7u, 11u,          // up, right+up, left+up, right+left+up
    1u, 8u, 9u, 12u,               // up+down, +right, +left, all four
);

// Whether the cell pass marked the edge cell at texel `c` a corner (#348)
fn marked_corner(c: vec2<i32>) -> bool {
    return (u32(edge_cell_at(c).z) & CORNER_BIT) != 0u;
}

// The line a neighbour at texel `c` lends a marked corner as an arm: its
// thinned line, if it's sure of one direction. A neighbour the cell pass
// marked a corner too is part of the same turn, not a straight arm, and
// lends none: two such cells side by side would each take the other for
// an arm and draw a corner apiece.
fn plain_arm(c: vec2<i32>) -> u32 {
    if marked_corner(c) {
        return NO_EDGE;
    }
    return thinned_slot(c);
}

// A marked corner's arms (#348), by right 1, left 2, down 4, up 8: the
// neighbours drawing the line that runs into it, - beside it, | above or
// below
fn marked_arms(c: vec2<i32>) -> u32 {
    var arms = 0u;
    if plain_arm(c + vec2<i32>(1, 0)) == 0u {
        arms |= 1u;
    }
    if plain_arm(c - vec2<i32>(1, 0)) == 0u {
        arms |= 2u;
    }
    if plain_arm(c + vec2<i32>(0, 1)) == 1u {
        arms |= 4u;
    }
    if plain_arm(c - vec2<i32>(0, 1)) == 1u {
        arms |= 8u;
    }
    return arms;
}

// The corner or tee `arms` make in a font with `corners` corner glyphs,
// or NO_EDGE where they make none it can draw (the cell keeps its line)
fn arm_shape(arms: u32, corners: u32) -> u32 {
    var table = ARM_GLYPHS;
    let shape = table[arms];
    if shape == NO_EDGE || shape < 4u || shape >= 4u + corners {
        return NO_EDGE;
    }
    return shape;
}

// The straight line the edge cell at texel `c` lends the line-end and gap
// corners as an arm (#637): its thinned line, and a marked corner's too
// where its own arms make no corner and it draws the line after all. A
// marked cell that does turn lends nothing: its arms point at its own
// neighbours' lines.
fn arm_slot(c: vec2<i32>, corners: u32) -> u32 {
    let line = thinned_slot(c);
    if line == NO_EDGE || !marked_corner(c) {
        return line;
    }
    if arm_shape(marked_arms(c), corners) != NO_EDGE {
        return NO_EDGE;
    }
    return line;
}

// How many of the two cells along a straight line drawn at texel `c`
// (`s`: 0 the line -, 1 the line |) lend it an arm: 1 where the line ends
fn along_arms(c: vec2<i32>, s: u32, corners: u32) -> u32 {
    let d = select(vec2<i32>(0, 1), vec2<i32>(1, 0), s == 0u);
    return u32(arm_slot(c + d, corners) == s) + u32(arm_slot(c - d, corners) == s);
}

// The arms bit for a step `d` to a neighbour: right 1, left 2, down 4, up 8
fn arm_bit(d: vec2<i32>) -> u32 {
    if d.x > 0 {
        return 1u;
    }
    if d.x < 0 {
        return 2u;
    }
    return select(8u, 4u, d.y > 0);
}

// A straight line that ends beside a perpendicular one (#637): the
// outline turns here, but the cell's edge pixels run mostly one way (the
// turn's other arm is in the next cell), so the cell pass's coherence
// test doesn't see a corner. A - drawn at `c` with only one - beside it
// ends here; a | above or below it is the turn's other arm, and the cell
// draws the corner (or the tee, with | on both sides). On the side the
// line ends, a neighbour drawing the perpendicular line that itself ends
// there turns into this cell and is an arm too: the | below a - that
// turned down becomes a tee when a - also runs into it from the side.
// A line that runs on through the cell never turns, so a line crossing
// another never becomes a cross, and a line with no perpendicular arm
// keeps its glyph. Returns the arms, 0 for none.
fn end_turn_arms(c: vec2<i32>, s: u32, corners: u32) -> u32 {
    let p = 1u - s;
    let along = select(vec2<i32>(0, 1), vec2<i32>(1, 0), s == 0u);
    let across = vec2<i32>(along.y, along.x);
    var arms = 0u;
    var ends = vec2<i32>(0);
    var count = 0u;
    if arm_slot(c + along, corners) == s {
        arms |= arm_bit(along);
        count += 1u;
    } else {
        ends = along;
    }
    if arm_slot(c - along, corners) == s {
        arms |= arm_bit(-along);
        count += 1u;
    } else {
        ends = -along;
    }
    if count != 1u {
        return 0u;
    }
    var crossing = 0u;
    if arm_slot(c + across, corners) == p {
        crossing |= arm_bit(across);
    }
    if arm_slot(c - across, corners) == p {
        crossing |= arm_bit(-across);
    }
    if crossing == 0u {
        return 0u;
    }
    // Past the end: nothing, or the perpendicular line turning in. A line
    // that goes on as a diagonal (a - into a \), or steps a cell across
    // (the same line in a cell diagonally past the end: a staircase, not a
    // turn), doesn't end here
    if thinned_slot(c + ends + across) == s || thinned_slot(c + ends - across) == s {
        return 0u;
    }
    let n = c + ends;
    let past = thinned_slot(n);
    if past != NO_EDGE {
        if arm_slot(n, corners) != p || along_arms(n, p, corners) != 1u {
            return 0u;
        }
        arms |= arm_bit(ends);
    }
    return arms | crossing;
}

// A corner in a gap (#637): a cell that draws no line, holding a sliver
// of edge under the threshold, where a - runs into it from the side and
// a | from above or below. That's the corner an outline turns in when it
// passes through too little of the cell to draw it, or when the - ends in
// one cell and the | starts in the cell diagonally past it: this is the
// cell beside both. Arms must lie the way they run into the cell, - beside
// it and | above or below, so the cell diagonally inside a box's corner
// (- above it, | beside it) is never one. A corner or a tee (- on both
// sides, where two edges meet a third), never a cross. Returns the arms,
// 0 for none.
fn gap_arms(c: vec2<i32>, corners: u32) -> u32 {
    if edge_cell_at(c).x > -0.125 {
        return 0u;
    }
    var across = 0u;
    if arm_slot(c + vec2<i32>(1, 0), corners) == 0u {
        across |= 1u;
    }
    if arm_slot(c - vec2<i32>(1, 0), corners) == 0u {
        across |= 2u;
    }
    var down = 0u;
    if arm_slot(c + vec2<i32>(0, 1), corners) == 1u {
        down |= 4u;
    }
    if arm_slot(c - vec2<i32>(0, 1), corners) == 1u {
        down |= 8u;
    }
    if across == 0u || down == 0u || (across == 3u && down == 12u) {
        return 0u;
    }
    // No neighbour runs the wrong way: a | beside the cell or a - above or
    // below it is a staircase line stepping past (a | above, the next | a
    // cell to the side), or the inside of a box's corner, not a turn
    if thinned_slot(c + vec2<i32>(1, 0)) == 1u || thinned_slot(c - vec2<i32>(1, 0)) == 1u
        || thinned_slot(c + vec2<i32>(0, 1)) == 0u || thinned_slot(c - vec2<i32>(0, 1)) == 0u {
        return 0u;
    }
    return across | down;
}

// The glyph the edge cell at texel `c` draws (#348): its thinned line, or
// the corner or tee its neighbours' lines meet in, in a cell the cell
// pass marked a corner, a line that ends beside a perpendicular one, or a
// gap between a - and a | (#637). `corners` is how many corner glyphs the
// drawing font has (0, 4 or 9; 0 too with `edge_corner = 0`); a meeting
// it has no glyph for keeps the line, which is what a set without corners
// always draws.
fn cornered_slot(c: vec2<i32>, corners: u32) -> u32 {
    let line = thinned_slot(c);
    if corners == 0u {
        return line;
    }
    if line == NO_EDGE {
        return arm_shape(gap_arms(c, corners), corners);
    }
    if marked_corner(c) {
        let shape = arm_shape(marked_arms(c), corners);
        if shape != NO_EDGE {
            return shape;
        }
    }
    if line <= 1u {
        let shape = arm_shape(end_turn_arms(c, line, corners), corners);
        if shape != NO_EDGE {
            return shape;
        }
    }
    return line;
}
