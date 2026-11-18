// ASCII tone statistics, rows (#289): each style slot's luminance spread,
// so `normalize` can stretch a dark prop's tones over the whole ramp.
//
// Glyph density follows luminance, so a dark prop maps to `.` and `-` and
// all but disappears. `normalize` stretches each slot's own range (slot 0
// is the whole frame, every other a per-entity variant) onto the ramp:
// a barrel's 0.03..0.2 becomes 0..1 and its staves read again.
//
// Two passes, both in the frame they serve, so a headless `flint render`
// is as deterministic as before (nothing is read back a frame late):
//
// - this one: one texel per (slot, lattice row, run of the row), the run
//   at y = run * STATS_ROWS + row (#337). It walks STATS_COLS /
//   STATS_CHUNKS samples of one row of a fixed STATS_COLS x STATS_ROWS
//   lattice over the picture, and for each sample whose mask slot is the
//   texel's slot adds the sample's display luminance (after the slot's
//   exposure) to a count, a sum and a sum of squares.
// - ascii_stats_resolve.wgsl: one texel per slot, the runs summed into
//   the range the slot normalises onto.
//
// Slots past the variant table's rows, and slots that don't normalise,
// write zeros and loop over nothing.

@group(1) @binding(0) var stats_in: texture_2d<f32>;
@group(1) @binding(1) var stats_mask: texture_2d<u32>;

fn stats_shown(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let c = clamp(textureLoad(stats_in, clamp(p, vec2<i32>(0), max_px), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, flint_to_display(c), u.srgb == 1u);
}

// The mask id under `p` (output pixels) the glyph pass finds its slot by:
// 0 with no variants, and outside `rect` (#507), where the slot being
// summed has no pixel
fn stats_id(p: vec2<f32>, rect: vec4<f32>) -> u32 {
    if u.variant_meta.x <= 1u || any(p < rect.xy) || any(p >= rect.zw) {
        return 0u;
    }
    let msize = vec2<i32>(textureDimensions(stats_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / u.size));
    return textureLoad(stats_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).r;
}

// Samples in a run
const STATS_RUN: u32 = STATS_COLS / STATS_CHUNKS;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<u32>(floor(in.position.xy));
    let slot = texel.x;
    if slot >= max(u.variant_meta.x, 1u) || slot_normalize(slot) <= 0.0 {
        return vec4<f32>(0.0);
    }
    let gain = exp2(slot_exposure(slot));
    let row = texel.y % STATS_ROWS;
    let run = STATS_COLS / STATS_CHUNKS;
    let first = (texel.y / STATS_ROWS) * run;
    let y = (f32(row) + 0.5) / f32(STATS_ROWS) * u.size.y;
    // Where the slot's pixels can be (#507): a variant's own bounds, any
    // variant's for the whole frame's (which is every pixel outside them).
    // A variant's run wholly outside its bounds meets none of its pixels:
    // the zeros the loop would sum
    var rect = u.mask_rect;
    if slot != 0u {
        rect = variants[slot].bounds;
    }
    let x0 = (f32(first) + 0.5) / f32(STATS_COLS) * u.size.x;
    let x1 = (f32(first + run - 1u) + 0.5) / f32(STATS_COLS) * u.size.x;
    if slot != 0u && (y < rect.y || y >= rect.w || x1 < rect.x || x0 >= rect.z) {
        return vec4<f32>(0.0);
    }
    // A step at a time over the run (#507), each step's loads independent
    // of one another: the mask ids, their slots, then the picture. Sample
    // after sample, a run waited on memory 60 times in a row, and that
    // wait, not the work, was the pass's time
    var s: array<u32, STATS_RUN>;
    for (var j = 0u; j < STATS_RUN; j++) {
        let p = vec2<f32>((f32(first + j) + 0.5) / f32(STATS_COLS) * u.size.x, y);
        s[j] = stats_id(p, rect);
    }
    var ours = 0u;
    for (var j = 0u; j < STATS_RUN; j++) {
        ours |= u32(slot_of(s[j]) == slot) << j;
    }
    if ours == 0u {
        return vec4<f32>(0.0);
    }
    // The sums in sample order: every sample loaded, a weight of 0
    // adding nothing
    var n = 0.0;
    var sum = 0.0;
    var sq = 0.0;
    for (var j = 0u; j < STATS_RUN; j++) {
        let p = vec2<f32>((f32(first + j) + 0.5) / f32(STATS_COLS) * u.size.x, y);
        let w = f32((ours >> j) & 1u);
        let l = flint_luma(exposed(stats_shown(vec2<i32>(floor(p))), gain));
        let lw = l * w;
        n += w;
        sum += lw;
        sq += l * lw;
    }
    return vec4<f32>(n, sum, sq, 0.0);
}
