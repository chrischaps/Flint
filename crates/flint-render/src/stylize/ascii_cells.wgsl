// ASCII cell pass (#265, #277): one fragment a glyph cell. It averages the
// cell's colour from the picture, picks the cell's ramp step, and writes
// both into the cell state the glyph pass draws from.
//
// Temporal stability (#277). Glyphs shimmer in motion because a cell's
// tone sits near a step boundary and the smallest change (a sub-pixel
// slide of the camera, a texel of specular) tips it to the neighbouring
// glyph and back. Three things hold it still, each its own parameter:
//
// - smoothing: the cell keeps part of last frame's colour (an exponential
//   average), except where the colour jumped (something new came into the
//   cell), which takes the new colour at once so nothing ghosts. The
//   style hands in this frame's weight, `smoothing` per 60 Hz frame raised
//   to the frame's length in 60 Hz frames (#342), so it settles in the same
//   time at any frame rate.
// - hysteresis: the cell keeps last frame's glyph while its tone is within
//   that many ramp steps of the glyph's band; it changes only when the
//   tone has clearly moved on.
// - dither: an ordered (Bayer) offset fixed to the grid, before the ramp,
//   so a smooth gradient breaks into a stable pattern rather than one
//   hard, crawling boundary.
//
// Last frame's state is read from the other of two textures the style
// ping-pongs; on a first frame (`u.history == 0`) none of it is read and
// the result is exactly the stateless style's.

@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var history: texture_2d<u32>;
// Each slot's tone range (#289, ascii_stats_resolve.wgsl); slot 0 is ours
@group(3) @binding(0) var tone_stats: texture_2d<f32>;

// The tone luminance of a cell colour: stretched over the frame's range
fn cell_luma(col: vec3<f32>) -> f32 {
    return stretched(luma(col), textureLoad(tone_stats, vec2<i32>(0, 0), 0), u.stretch);
}

// Colour change (in luminance) over which smoothing lets go: below LO the
// cell averages fully, above HI it takes the new colour.
const SNAP_LO: f32 = 0.06;
const SNAP_HI: f32 = 0.2;

fn shown(p: vec2<i32>) -> vec3<f32> {
    let max_px = vec2<i32>(u.size) - vec2<i32>(1);
    let c = clamp(textureLoad(input_texture, clamp(p, vec2<i32>(0), max_px), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c, to_display(c), u.srgb == 1u);
}

// The ramp step a tone draws, after the optional tone bands.
fn step_of(tone_in: f32, n: u32) -> u32 {
    return min(u32(banded(tone_in) * f32(n)), n - 1u);
}

// A 4x4 Bayer threshold in [0, 1), fixed to the cell grid.
fn bayer(c: vec2<i32>) -> f32 {
    let m = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let i = vec2<u32>(c & vec2<i32>(3));
    return (f32(m[i.y * 4u + i.x]) + 0.5) / 16.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<u32> {
    let index = vec2<i32>(floor(in.position.xy));
    let cell = vec2<f32>(index - vec2<i32>(CELL_OFFSET));
    let cell_min = cell * u.cell + u.origin;

    // The cell's colour: the mean of a 4x4 lattice over a footprint
    // `footprint` cells wide, centred on the cell
    let foot = u.cell * max(u.footprint, 1.0);
    let centre = cell_min + 0.5 * u.cell;
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let q = centre + ((vec2<f32>(f32(i), f32(j)) + 0.5) / 4.0 - 0.5) * foot;
            sum += shown(vec2<i32>(floor(q)));
        }
    }
    // Exposure (#289) first, so the history and the glyph pass see the
    // colour the cell is drawn in
    var col = exposed(sum / 16.0, exp2(u.exposure));

    let n = max(u.ramp_len, 1u);
    var kept = -1;
    if u.history == 1u {
        let prev = textureLoad(history, index, 0);
        let prev_col = vec3<f32>(prev.rgb) / UNORM16;
        let jump = abs(cell_luma(col) - cell_luma(prev_col));
        let w = clamp(u.smoothing, 0.0, 0.999) * (1.0 - smoothstep(SNAP_LO, SNAP_HI, jump));
        col = mix(col, prev_col, w);
        kept = i32(min(prev.a, n - 1u));
    }

    // The tone, dithered on the grid by up to one band
    let bands = select(f32(n), u.levels, u.levels >= 1.0);
    let t = tone(cell_luma(col)) + (bayer(index) - 0.5) * u.dither / bands;
    var step = step_of(t, n);
    if kept >= 0 && u.hysteresis > 0.0 {
        // Keep last frame's glyph if a tone within the margin would draw it
        let margin = u.hysteresis / bands;
        let k = u32(kept);
        if k >= step_of(t - margin, n) && k <= step_of(t + margin, n) {
            step = k;
        }
    }

    let stored = vec3<u32>(round(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)) * UNORM16));
    return vec4<u32>(stored, step);
}
