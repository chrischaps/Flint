// ASCII dissolve order (#278) and its per-prop rank (#398).
// ascii_common.wgsl comes first. The glyph pass and the two passes that
// run before it (ascii_dissolve_order.wgsl, ascii_dissolve_rank.wgsl)
// include it.
//
// A cell turns into a glyph once the blend passes its place in the
// dissolve order: patch noise a few cells across plus a per-cell hash. On
// average the share of turned cells is the blend, but a prop of few cells
// holds only a few patches: a 6x5-cell prop at blend 0.5 turned anywhere
// from 30 to 73% of its cells, by where it sat on the noise (#398). So the
// glyph pass compares the blend with the cell's *rank* in the order among
// its own variant's cells nearby, not with the order itself:
//
//   rank = (cells of the variant within DISSOLVE_REACH whose order is
//           lower, + 0.5) / (cells of the variant within DISSOLVE_REACH)
//
// A prop no wider or taller than DISSOLVE_REACH + 1 cells is wholly in
// each of its cells' windows, so its ranks are 0.5/n, 1.5/n, ... and
// exactly round(blend x n) of its n cells turn, wherever it sits. A larger
// prop is ranked window by window, which flattens the noise's slow swells
// and keeps its patches. The order is unchanged, so the patches are the
// same ones, a ramp turns cells in the same sequence and never shimmers.
//
// Each dissolving variant has a band of rows on its own grid, laid out as
// the cell state is (CELL_OFFSET): `u.dissolve[i]` = (slot, first row,
// rows, columns), and its row's `dissolve_row` is the first row + 1.

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
// half per-cell hash (so a patch's edge is ragged). The sum of two
// near-uniform values is triangular; its CDF maps it back to uniform.
fn dissolve_order(cell: vec2<f32>) -> f32 {
    let n = value_noise(cell * 0.3) * 0.65 + value_noise(cell * 0.6 + vec2<f32>(31.0, 7.0)) * 0.35;
    // Value noise piles up in the middle; spread it back toward 0..1
    let patches = clamp((n - 0.5) * 2.0 + 0.5, 0.0, 1.0);
    let s = 0.5 * (patches + cell_hash(cell + vec2<f32>(17.0, 5.0)));
    let u = select(1.0 - 2.0 * (1.0 - s) * (1.0 - s), 2.0 * s * s, s < 0.5);
    return clamp(u, 0.0, 0.9999);
}

// Cells either side a cell's rank counts (#398): a 17x17 window
const DISSOLVE_REACH: i32 = 8;

// What a texel of the order or rank band holds where the cell isn't the
// variant's
const NOT_MEMBER: f32 = -1.0;

// The dissolve band holding texel row `y`, or -1
fn dissolve_band(texel: vec2<i32>) -> i32 {
    for (var i = 0u; i < min(u.dissolve_meta.x, MAX_DISSOLVE); i++) {
        let d = u.dissolve[i];
        if texel.y >= i32(d.y) && texel.y < i32(d.y + d.z) && texel.x < i32(d.w) {
            return i32(i);
        }
    }
    return -1;
}
