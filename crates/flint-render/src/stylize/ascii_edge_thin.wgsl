// ASCII edges (#267), the edge thin pass (#910), first of two: each edge
// cell's thinned line, and for a cell the edge cell pass marked a corner
// (#348), the arms its neighbours' lines make. One fragment a texel of a
// texture one cell bigger than the edge cells on every side (THIN_PAD), so
// the corner pass (ascii_edge_corners.wgsl) reads its neighbours past the
// grid's border as these functions see them. ascii_edge_slot.wgsl comes
// first; it says how the texel packs. Until #910 the glyph pass and the
// edge view ran all of this per pixel: under FXC's full inlining the
// corners were about 45 copies of `thinned_slot`, and the glyph shader
// took 25-66 s to compile on DX12.

@group(0) @binding(0) var edge_cells: texture_2d<f32>;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) u32 {
    let c = vec2<i32>(floor(in.position.xy)) - vec2<i32>(THIN_PAD);
    let marked = marked_corner(c);
    var arms = 0u;
    if marked {
        arms = marked_arms(c);
    }
    return pack_thin(thinned_slot(c), marked, arms);
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
