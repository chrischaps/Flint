// ASCII dissolve rank pass (#398): ascii_common.wgsl and
// ascii_dissolve.wgsl come first.

@group(1) @binding(0) var order_cells: texture_2d<f32>;

// Each member cell's rank in the order among its variant's cells within
// DISSOLVE_REACH, (lower + 0.5) / count; ties go to the earlier cell in
// row order, so no two cells share a rank.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(in.position.xy));
    let band = dissolve_band(texel);
    if band < 0 {
        return vec4<f32>(NOT_MEMBER);
    }
    let own = textureLoad(order_cells, texel, 0).r;
    if own < 0.0 {
        return vec4<f32>(NOT_MEMBER);
    }
    let d = u.dissolve[band];
    let lo = vec2<i32>(0, i32(d.y));
    let hi = vec2<i32>(i32(d.w), i32(d.y + d.z)) - 1;
    let first = max(texel - vec2<i32>(DISSOLVE_REACH), lo);
    let last = min(texel + vec2<i32>(DISSOLVE_REACH), hi);
    var count = 0.0;
    var lower = 0.0;
    for (var y = first.y; y <= last.y; y++) {
        for (var x = first.x; x <= last.x; x++) {
            let o = textureLoad(order_cells, vec2<i32>(x, y), 0).r;
            if o < 0.0 {
                continue;
            }
            count += 1.0;
            let before = y < texel.y || (y == texel.y && x < texel.x);
            if o < own || (o == own && before) {
                lower += 1.0;
            }
        }
    }
    return vec4<f32>((lower + 0.5) / count, 0.0, 0.0, 0.0);
}
