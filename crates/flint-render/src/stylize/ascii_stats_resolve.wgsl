// ASCII tone statistics, resolve (#289): one texel per style slot, the
// rows (fs_rows below sums each from its runs, #337) summed into the
// range the slot normalises onto: mean -/+ STATS_SPREAD standard
// deviations. That is a percentile without a histogram: a highlight or a crack moves it far less than it
// would a min and max. Written (lo, hi, mean, 1); zeros for a slot
// with no samples or no `normalize`, which the readers treat as "leave
// the tone alone".

@group(1) @binding(0) var stats_rows: texture_2d<f32>;

// The lattice rows `slot`'s runs can be other than zeros in, [x, y): all of
// them for the whole frame's; for a variant's, those that can meet its
// bounds (#507, ascii_stats.wgsl), a row more on each side. Row r samples
// y = (r + 0.5) / ROWS * h.
fn stats_row_span(slot: u32) -> vec2<i32> {
    if slot == 0u {
        return vec2<i32>(0, i32(STATS_ROWS));
    }
    let rows = f32(STATS_ROWS) / u.size.y;
    let bounds = variants[slot].bounds;
    return vec2<i32>(
        max(i32(floor(bounds.y * rows - 0.5)) - 1, 0),
        min(i32(ceil(bounds.w * rows - 0.5)) + 1, i32(STATS_ROWS)),
    );
}

// One texel per (slot, lattice row): the row's STATS_CHUNKS runs that
// ascii_stats.wgsl wrote at y = run * STATS_ROWS + row, summed. Three
// short passes, not two long ones: a fragment's loop is serial, so the
// sums go 20, 16 and 180 deep rather than 320 and 180 (#337).
@fragment
fn fs_rows(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(in.position.xy));
    if u32(texel.x) >= max(u.variant_meta.x, 1u) || slot_normalize(u32(texel.x)) <= 0.0 {
        return vec4<f32>(0.0);
    }
    let span = stats_row_span(u32(texel.x));
    if texel.y < span.x || texel.y >= span.y {
        return vec4<f32>(0.0);
    }
    var acc = vec3<f32>(0.0);
    for (var r = 0; r < i32(STATS_CHUNKS); r++) {
        acc += textureLoad(stats_rows, vec2<i32>(texel.x, r * i32(STATS_ROWS) + texel.y), 0).rgb;
    }
    return vec4<f32>(acc, 0.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let slot = u32(floor(in.position.x));
    if slot >= max(u.variant_meta.x, 1u) || slot_normalize(slot) <= 0.0 {
        return vec4<f32>(0.0);
    }
    // Only the rows that can be other than zeros, added in order (#507);
    // eight loads ahead of their adds, so the fragment waits on memory once
    // a block rather than once a row
    let span = stats_row_span(slot);
    var acc = vec3<f32>(0.0);
    for (var r0 = span.x; r0 < span.y; r0 += 8) {
        var t: array<vec3<f32>, 8>;
        for (var k = 0; k < 8; k++) {
            t[k] = textureLoad(stats_rows, vec2<i32>(i32(slot), min(r0 + k, span.y - 1)), 0).rgb;
        }
        for (var k = 0; k < 8; k++) {
            if r0 + k < span.y {
                acc += t[k];
            }
        }
    }
    if acc.x < 1.0 {
        return vec4<f32>(0.0);
    }
    let mean = acc.y / acc.x;
    let sd = sqrt(max(acc.z / acc.x - mean * mean, 0.0));
    let lo = clamp(mean - STATS_SPREAD * sd, 0.0, 1.0);
    let hi = clamp(mean + STATS_SPREAD * sd, 0.0, 1.0);
    return vec4<f32>(lo, hi, mean, 1.0);
}
