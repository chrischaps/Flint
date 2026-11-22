// Tron (#746; render mode 4): the scene dimmed to a ghost, its depth and
// luminance edges glowing, and a world-space grid rebuilt from depth that
// rides whatever surface it lands on (a sea's swells).
//
// p0 = the glow's rgb, edge strength
// p1 = the scene's dim, the sky's dim, grid strength, grid spacing (m)
// p2 = pulse speed, how far the grid reaches (m)

// The shorter of a pixel's two one-sided steps to a neighbour's world
// position: across a silhouette one side lands on another surface, and the
// near side's step is the pixel's own footprint
fn tron_step(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return select(b, a, dot(a, a) < dot(b, b));
}

// One set of grid lines along one world axis, `cell` metres apart and
// `half_w` metres either side of the line, blurred over `soft` metres,
// where a pixel spans `fp` metres of the axis (#1181). A line drawn no
// thinner than a pixel dims by as much as it was widened, so it thins with
// distance instead of shimmering; once a pixel spans most of a cell the
// lines can't be told apart and fade to the cell's average.
fn tron_lines(c: f32, cell: f32, half_w: f32, soft: f32, fp: f32) -> f32 {
    let d = abs(fract(c / cell + 0.5) - 0.5) * cell;
    let w = max(half_w, fp);
    let blur = max(soft, fp);
    let line = (1.0 - smoothstep(w - blur * 0.5, w + blur * 0.5, d)) * (half_w / w);
    return mix(line, 2.0 * half_w / cell, smoothstep(0.3, 0.8, fp / cell));
}

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
    let uv_l = uv - vec2<f32>(px.x, 0.0);
    let uv_r = uv + vec2<f32>(px.x, 0.0);
    let uv_u = uv - vec2<f32>(0.0, px.y);
    let uv_d = uv + vec2<f32>(0.0, px.y);
    let depth_l = depth_at(uv_l);
    let depth_r = depth_at(uv_r);
    let depth_u = depth_at(uv_u);
    let depth_d = depth_at(uv_d);
    let lin_c = linear_depth(depth_c);
    let lin_l = linear_depth(depth_l);
    let lin_r = linear_depth(depth_r);
    let lin_u = linear_depth(depth_u);
    let lin_d = linear_depth(depth_d);
    // Depth discontinuities (silhouettes): where the depth bends, not where
    // it merely changes. A plane's inverse depth runs straight across the
    // screen, so its second difference is zero however steeply its depth
    // climbs; a ground plane seen at a grazing angle used to read as a
    // silhouette at every pixel and fill with glow (#1181). A step of 5% of
    // the view distance on one side reads 1, as the first difference did,
    // and nearer than a metre the step is counted in metres, as it was.
    let bend = abs(lin_c / lin_l + lin_c / lin_r - 2.0) + abs(lin_c / lin_u + lin_c / lin_d - 2.0);
    let d_edge = bend * lin_c / max(lin_c * 0.05, 0.05);

    // Luminance gradient assist picks up interior detail (foam lace, logs).
    let lum_l = flint_luma(picture(uv_l));
    let lum_r = flint_luma(picture(uv_r));
    let lum_u = flint_luma(picture(uv_u));
    let lum_d = flint_luma(picture(uv_d));
    let l_edge = (abs(lum_r - lum_l) + abs(lum_d - lum_u)) * 2.0;

    let edge = smoothstep(0.25, 0.9, d_edge + l_edge);

    // World-space grid that follows the displaced surface: lines every
    // `spacing` metres and stronger ones every five of them, each as wide
    // as the pixel's footprint allows.
    let wp = world_pos_from_depth(uv, depth_c);
    let step_x = tron_step(world_pos_from_depth(uv_r, depth_r) - wp, wp - world_pos_from_depth(uv_l, depth_l));
    let step_y = tron_step(world_pos_from_depth(uv_d, depth_d) - wp, wp - world_pos_from_depth(uv_u, depth_u));
    let fp = abs(step_x.xz) + abs(step_y.xz);
    let dist = length(wp - r.camera_pos);
    let fade = exp(-dist / max(r.p2.y, 0.01));
    let spacing = max(r.p1.w, 0.01);
    let major = spacing * 5.0;
    let line1 = max(
        tron_lines(wp.x, spacing, 0.02 * spacing, 0.03 * spacing, fp.x),
        tron_lines(wp.z, spacing, 0.02 * spacing, 0.03 * spacing, fp.y),
    );
    let line2 = max(
        tron_lines(wp.x, major, 0.0113 * major, 0.0175 * major, fp.x),
        tron_lines(wp.z, major, 0.0113 * major, 0.0175 * major, fp.y),
    );
    let pulse = 0.75 + 0.25 * sin(r.time * r.p2.x - dist * 0.15);
    let grid = (line1 * 0.35 + line2 * 0.7) * fade * pulse;

    out_col += glow * (edge * r.p0.w + grid * r.p1.z);
    return shown(out_col, uv);
}
