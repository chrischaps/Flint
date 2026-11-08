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
//
// Reprojection (#344, `temporal = "reproject"`). Kept in screen space, a
// cell's past is whatever was on screen there last frame: under a fast
// pan that is another surface, the colour snaps and hysteresis has
// nothing to hold. Reprojected, the cell's centre is unprojected through
// the scene depth and projected with last frame's camera, and the past is
// read from the cell that point fell in: a glyph travels with its
// surface. Each cell also stores its centre's view depth (the second
// target); a past whose depth disagrees with where the surface should
// have been (a disocclusion: it was behind something) is not the cell's,
// and the cell starts fresh. Camera motion only: Flint writes no motion
// vectors, so a moving object's cells read the past behind it and the
// depth test turns most of that away.

@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var history: texture_2d<u32>;
@group(2) @binding(1) var history_depth: texture_2d<f32>;
@group(2) @binding(2) var scene_depth: texture_depth_2d;
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

// The scene's view depth (clip w) at output pixel `px`, and the world
// point there. The sky is cleared to depth 1, which unprojects to the far
// plane by a w f32 rounds to 0; stop just short of it (ascii_edges.wgsl).
struct Surface {
    world: vec3<f32>,
    w: f32,
};

fn surface_at(px: vec2<f32>) -> Surface {
    let dsize = vec2<i32>(textureDimensions(scene_depth));
    let uv = px / u.size;
    let q = clamp(vec2<i32>(floor(uv * vec2<f32>(dsize))), vec2<i32>(0), dsize - vec2<i32>(1));
    let d = min(textureLoad(scene_depth, q, 0), 0.99999);
    let h = u.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let hw = select(h.w, 1e-6, abs(h.w) < 1e-6);
    // inv_view_proj * ndc is the world point over the clip w
    return Surface(h.xyz / hw, 1.0 / hw);
}

// A past whose stored depth is further than this from where the surface
// should have been, relative, belongs to something else (#344)
const DISOCCLUSION: f32 = 0.1;

struct CellOut {
    @location(0) state: vec4<u32>,
    // The cell centre's view depth, the next frame's disocclusion test
    @location(1) depth: f32,
};

@fragment
fn fs_main(in: VertexOutput) -> CellOut {
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
    let here = surface_at(centre);
    // Where to read the past: the same cell, or (reprojected) the cell the
    // surface at this cell's centre was in last frame
    var past = index;
    var has_past = u.history == 1u;
    if has_past && u.reproject.x == 1u {
        let clip = u.prev_view_proj * vec4<f32>(here.world, 1.0);
        let grid = vec2<i32>(textureDimensions(history));
        if clip.w > 1e-4 {
            let ndc = clip.xy / clip.w;
            let px = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * u.size;
            past = vec2<i32>(floor((px - u.origin) / u.cell)) + vec2<i32>(CELL_OFFSET);
            let inside = all(past >= vec2<i32>(0)) && all(past < grid);
            let was = textureLoad(history_depth, clamp(past, vec2<i32>(0), grid - vec2<i32>(1)), 0).r;
            let agree = abs(was - clip.w) <= DISOCCLUSION * max(was, clip.w);
            has_past = inside && agree;
        } else {
            has_past = false;
        }
    }
    if has_past {
        let prev = textureLoad(history, past, 0);
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
    return CellOut(vec4<u32>(stored, step), here.w);
}
