// ASCII tone statistics, resolve (#289): one texel per style slot, the
// rows (fs_rows below sums each from its runs, #337) summed into the
// range the slot normalises onto: mean -/+ STATS_SPREAD standard
// deviations. That is a percentile without a histogram: a highlight or a crack moves it far less than it
// would a min and max. Written (lo, hi, mean, 1); zeros for a slot
// with no samples or no `normalize`, which the readers treat as "leave
// the tone alone".

@group(1) @binding(0) var stats_rows: texture_2d<f32>;

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
    var acc = vec3<f32>(0.0);
    for (var r = 0u; r < STATS_ROWS; r++) {
        acc += textureLoad(stats_rows, vec2<i32>(i32(slot), i32(r)), 0).rgb;
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
