// Tron (#746; render mode 4): the scene dimmed to a ghost, its depth and
// luminance edges glowing, and a world-space grid rebuilt from depth that
// rides whatever surface it lands on (a sea's swells).
//
// p0 = the glow's rgb, edge strength
// p1 = the scene's dim, the sky's dim, grid strength, grid spacing (m)
// p2 = pulse speed, how far the grid reaches (m)

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let mapped = picture(uv);
    let glow = r.p0.rgb;
    let depth_c = depth_at(uv);

    if (is_sky_depth(depth_c)) {
        // Sky: keep only the dim ghost.
        return shown(mapped * r.p1.y, uv);
    }
    var out_col = mapped * r.p1.x;

    let px = r.texel_size;
    let lin_c = linear_depth(depth_c);
    let lin_l = linear_depth(depth_at(uv - vec2<f32>(px.x, 0.0)));
    let lin_r = linear_depth(depth_at(uv + vec2<f32>(px.x, 0.0)));
    let lin_u = linear_depth(depth_at(uv - vec2<f32>(0.0, px.y)));
    let lin_d = linear_depth(depth_at(uv + vec2<f32>(0.0, px.y)));
    // Depth discontinuities, normalized by view distance (silhouettes).
    let d_edge = (abs(lin_r - lin_l) + abs(lin_d - lin_u)) / max(lin_c * 0.05, 0.05);

    // Luminance gradient assist picks up interior detail (foam lace, logs).
    let lum_l = flint_luma(picture(uv - vec2<f32>(px.x, 0.0)));
    let lum_r = flint_luma(picture(uv + vec2<f32>(px.x, 0.0)));
    let lum_u = flint_luma(picture(uv - vec2<f32>(0.0, px.y)));
    let lum_d = flint_luma(picture(uv + vec2<f32>(0.0, px.y)));
    let l_edge = (abs(lum_r - lum_l) + abs(lum_d - lum_u)) * 2.0;

    let edge = smoothstep(0.25, 0.9, d_edge + l_edge);

    // World-space grid that follows the displaced surface: lines every
    // `spacing` metres and stronger ones every five of them.
    let wp = world_pos_from_depth(uv, depth_c);
    let dist = length(wp - r.camera_pos);
    let fade = exp(-dist / max(r.p2.y, 0.01));
    let spacing = max(r.p1.w, 0.01);
    let g1 = abs(fract(wp.xz / spacing) - 0.5) * 2.0;
    let g2 = abs(fract(wp.xz / (spacing * 5.0)) - 0.5) * 2.0;
    let line1 = max(smoothstep(0.93, 0.99, g1.x), smoothstep(0.93, 0.99, g1.y));
    let line2 = max(smoothstep(0.96, 0.995, g2.x), smoothstep(0.96, 0.995, g2.y));
    let pulse = 0.75 + 0.25 * sin(r.time * r.p2.x - dist * 0.15);
    let grid = (line1 * 0.35 + line2 * 0.7) * fade * pulse;

    out_col += glow * (edge * r.p0.w + grid * r.p1.z);
    return shown(out_col, uv);
}
