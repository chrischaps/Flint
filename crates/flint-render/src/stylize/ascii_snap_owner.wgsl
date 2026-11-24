// ASCII snap owner pass (#929): which snapping variant holds each cell of
// each snapping grid, once a cell. ascii_common.wgsl comes first.
//
// A cell silhouette (#338, `silhouette = "cell"`) draws a cell of its grid
// whole when the variant covers `cell_share` of the cell's 16 lattice
// samples. Every pixel of the glyph pass asked it for itself, per grid, with
// 16 mask loads and a chain of branches, and FXC (DX12's compiler, on the
// CPU) inlined it into both of the glyph shader's entry points: about 6 s of
// a compile. The answer is the cell's, so it is decided here, and the glyph
// pass reads one texel a grid (`held_cell`).
//
// Each grid with a snapping variant has a band of rows, the cells near its
// variants' bounds (`OwnerBand`, `assign_owner_bands`); a snapping row's
// `owner` (and `u.snap_bands` for `snap_grids`) says where: x, y = the
// grid's cell at the band's first texel, z = its first row + 1, w = columns
// | rows << 16. A band is drawn scissored with its first variant's slot as
// the instance, so the pass reads the grid from that row. Each texel holds
//
//   slot | (the mask's blend at the variant's first sample) << 16
//
// slot 0 for no snapping variant. The rule, and its arithmetic, is the one
// the glyph pass used: the same lattice, the same order, the same tie.

// The variant cell pass's group: the picture (unread) and the mask
@group(1) @binding(0) var owner_input: texture_2d<f32>;
@group(1) @binding(1) var flint_mask_texture: texture_2d<u32>;

struct OwnerVertex {
    @builtin(position) position: vec4<f32>,
    // The slot of the band's first variant
    @location(0) @interpolate(flat) slot: u32,
};

@vertex
fn vs_owner(@builtin(vertex_index) vi: u32, @builtin(instance_index) slot: u32) -> OwnerVertex {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: OwnerVertex;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.slot = slot;
    return out;
}

// As the glyph pass's `mask_texel`
fn owner_mask_texel(p: vec2<f32>) -> vec2<u32> {
    return flint_mask_load(flint_mask_texel_of(p, u.size));
}

// The snapping variant on the band's grid that holds the cell: of the
// variants on that grid that snap, the one with the most of the cell's 16
// lattice samples (the first met on a tie), if it has at least its
// `cell_share` of them. One pass, in registers: it counts the first two
// such variants met in the cell, which is every one unless three snapping
// variants of one grid meet in a cell (the third is then left out).
@fragment
fn fs_main(in: OwnerVertex) -> @location(0) vec4<u32> {
    let grid = variants[in.slot];
    let cell = grid.cell;
    let origin = grid.origin;
    let band = grid.owner;
    let texel = vec2<i32>(floor(in.position.xy));
    let c = vec2<f32>(band.xy + vec2<i32>(texel.x, texel.y - (band.z - 1)));
    let cell_min = c * cell + origin;
    var a = 0u;
    var a_n = 0u;
    var a_blend = 0u;
    var b = 0u;
    var b_n = 0u;
    var b_blend = 0u;
    for (var k = 0; k < 16; k++) {
        let m = owner_mask_texel(lattice(cell_min, cell, k));
        // Most samples are unstyled: no table reads for them
        var s = 0u;
        if m.x != 0u {
            s = slot_of(m.x);
        }
        if s == 0u {
        } else if s == a {
            a_n += 1u;
        } else if s == b {
            b_n += 1u;
        } else if snaps(s) && all(variants[s].cell == cell) && all(variants[s].origin == origin) {
            if a == 0u {
                a = s;
                a_n = 1u;
                a_blend = m.y;
            } else if b == 0u {
                b = s;
                b_n = 1u;
                b_blend = m.y;
            }
        }
    }
    if b_n > a_n {
        a = b;
        a_n = b_n;
        a_blend = b_blend;
    }
    var held = 0u;
    if a != 0u && f32(a_n) + 1e-3 >= variants[a].cell_share * 16.0 {
        held = a | (a_blend << 16u);
    }
    return vec4<u32>(held, 0u, 0u, 0u);
}
