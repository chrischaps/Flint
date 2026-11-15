// ASCII edges (#267), the edge corner pass (#910), second of two: the
// glyph each edge cell draws (#348, #637), decided once per cell. One
// fragment a cell of the edge cell texture (the same grid, CELL_OFFSET
// in), reading the thin pass's texels (ascii_edge_thin.wgsl) for the
// lines and marked corners and writing the slots `pack_slots` packs; the
// glyph pass (ascii.wgsl) and the edge debug view (ascii_edge_view.wgsl,
// #349) read that one texel, so the view shows the decision the glyphs
// make. ascii_edge_slot.wgsl comes first.

@group(0) @binding(0) var thin_cells: texture_2d<u32>;
@group(1) @binding(0) var edge_cells: texture_2d<f32>;

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

// Temporal stability (#477). Which corner or tee a cell draws is decided
// from its neighbours' lines a cell or two away, and one of them coming
// and going for a frame turned a ┌ into a ├ and back. A cell that draws
// the same line as last frame changes the shape it draws only when two
// frames running find the new one; a cell whose line comes, goes or
// turns takes this frame's at once, so an outline moving a cell a frame
// is never held back. The history flag and last frame's slots are the
// thin pass's (ascii_edge_thin.wgsl).
struct ThinUniforms {
    history: u32,
    reach: f32,
    _pad: vec2<u32>,
};

@group(2) @binding(0) var<uniform> t: ThinUniforms;
@group(3) @binding(0) var past_slots: texture_2d<u32>;

// The shape to draw: this frame's if it is what the cell drew or what
// last frame found too, else what the cell drew
fn settled(found: u32, drawn: u32, found_before: u32) -> u32 {
    if found == drawn || found == found_before {
        return found;
    }
    return drawn;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) u32 {
    let c = vec2<i32>(floor(in.position.xy));
    let line = thinned_slot(c);
    let four = cornered_slot(c, 4u);
    let nine = cornered_slot(c, 9u);
    var drawn_four = four;
    var drawn_nine = nine;
    // `hysteresis = 0` (reach 0) holds nothing
    if t.history == 1u && t.reach > 0.0 && line != NO_EDGE {
        let was = textureLoad(past_slots, c, 0).x;
        if (was & 15u) == line {
            drawn_four = settled(four, (was >> 4u) & 15u, (was >> 12u) & 15u);
            drawn_nine = settled(nine, (was >> 8u) & 15u, (was >> 16u) & 15u);
        }
    }
    return pack_slots(line, drawn_four, drawn_nine, four, nine);
}

// The thin pass's texel for the edge cell at texel `c`, which may lie a
// cell past the grid
fn thin_at(c: vec2<i32>) -> u32 {
    let cmax = vec2<i32>(textureDimensions(thin_cells)) - vec2<i32>(1);
    return textureLoad(thin_cells, clamp(c + vec2<i32>(THIN_PAD), vec2<i32>(0), cmax), 0).x;
}

// The line glyph slot the edge cell at texel `c` draws, thinned, or
// NO_EDGE (ascii_edge_thin.wgsl's `thinned_slot`)
fn thinned_slot(c: vec2<i32>) -> u32 {
    return thin_line(thin_at(c));
}

// Whether the cell pass marked the edge cell at texel `c` a corner (#348)
fn marked_corner(c: vec2<i32>) -> bool {
    return thin_marked(thin_at(c));
}

// A marked corner's arms (#348), by right 1, left 2, down 4, up 8
// (ascii_edge_thin.wgsl's `marked_arms`)
fn marked_arms(c: vec2<i32>) -> u32 {
    return thin_arms(thin_at(c));
}

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
