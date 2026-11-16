// Underwater (#746; render mode 5): the moments a swell buries the eye. Not
// a tear: its mask is the WATERLINE, tested per pixel against the flat
// plane y = camera y + `depth` (the signed eye depth a game's script gives
// it). Cel grammar throughout: banded absorption instead of an exponential
// gradient, two-hard-step light shafts, and a flat white foam lip on the
// line. The sea's underside agrees through the ocean's `tint` (its
// `OceanTint::underside` settings), which the game drives beside it. The
// style draws its own blend: the refraction sway grows with it.
//
// p0 = eye depth (m, + = under), churn, daylight, glow (bioluminescence)
// p1 = the blend, sway

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let mix_amt = clamp(r.p1.x, 0.0, 1.0);
    let eye_depth = r.p0.x;
    let churn = clamp(r.p0.y, 0.0, 1.0);
    let daylight = clamp(r.p0.z, 0.0, 1.0);
    let biolum = clamp(r.p0.w, 0.0, 1.0);
    let aspect = r.aspect;
    let t = r.time;

    // ── Refraction sway, once genuinely submerged: a straddle frame keeps
    // its above-water half rock steady.
    let amp = 0.004 * mix_amt * r.p1.y * smoothstep(0.1, 0.3, eye_depth);
    let uv = in.uv + vec2<f32>(
        sin(t * 0.7 + in.uv.y * 3.0),
        cos(t * 0.9 + in.uv.x * 2.5) * 0.6
    ) * amp;
    let mapped = picture(uv);

    // ── Waterline: near-plane world point vs the wobbled water plane. The
    // near plane is only ~0.15 m tall in world units, so wobble amplitudes
    // are millimeters — enough to break the ruler-straight edge.
    let nearw = world_pos_from_depth(uv, 0.0);
    let wobble = (value_noise(vec2<f32>(
        uv.x * aspect * 9.0 + t * 1.3,
        t * 0.7)) - 0.5) * (0.006 + 0.010 * churn);
    let delta = (r.camera_pos.y + eye_depth + wobble) - nearw.y;
    let under = smoothstep(-0.0015, 0.0015, delta);
    if (under <= 0.0) {
        return shown(mapped, uv);
    }

    // ── Water column length behind this pixel. Sky depth only survives a
    // sea's far fade, so treat it as a full column of water.
    let depth = depth_at(uv);
    var dist = 60.0;
    if (!is_sky_depth(depth)) {
        dist = length(world_pos_from_depth(uv, depth) - r.camera_pos);
    }

    // ── Banded absorption: 3 hard cel steps toward flat murk. Turbidity
    // (churn) and depth pull the bands closer; night kills the value.
    let absorb = 0.22 + 0.45 * churn + 0.15 * clamp(eye_depth, 0.0, 1.0);
    let tt = 1.0 - exp(-dist * absorb);
    let ts = tt * 3.0;
    let tq = clamp((floor(ts) + smoothstep(0.88, 0.97, fract(ts))) / 3.0, 0.0, 1.0);

    let light = 0.12 + 0.88 * daylight;
    let murk_near = mix(vec3<f32>(0.07, 0.30, 0.46), vec3<f32>(0.08, 0.30, 0.30), churn);
    let murk_far = mix(vec3<f32>(0.015, 0.10, 0.24), vec3<f32>(0.02, 0.10, 0.14), churn);

    let tint = mapped * vec3<f32>(0.45, 0.82, 1.05);
    var wcol = mix(tint, mix(murk_near, murk_far, tq) * light, tq);

    // ── Snell window: the hard-rimmed bright circle straight overhead.
    let vdir = normalize(nearw - r.camera_pos);
    let snell = smoothstep(0.62, 0.68, vdir.y);
    wcol += snell * daylight * vec3<f32>(0.18, 0.28, 0.30);

    // ── Light shafts: 1D noise over a slanted screen coordinate, quantized
    // to two hard steps, drifting slowly. Fade down-screen, drown in churn.
    let sc = uv.x * aspect * 2.2 - uv.y * 0.8 + t * 0.06;
    let sn = value_noise(vec2<f32>(sc * 7.0, t * 0.11));
    let shaft = (step(0.68, sn) * 0.5 + step(0.80, sn) * 0.5)
        * (1.0 - uv.y * 0.75) * daylight * (1.0 - 0.7 * churn);
    wcol += shaft * tq * vec3<f32>(0.05, 0.11, 0.13);

    // ── Bioluminescent specks (night foam glow carried underwater).
    if (biolum > 0.001) {
        let sp = uv * vec2<f32>(aspect, 1.0) * 42.0
            + vec2<f32>(t * 0.15, t * 0.35);
        let h = hash2(floor(sp));
        let dotd = length(fract(sp) - 0.5);
        let tw = 0.5 + 0.5 * sin(t * 3.0 + h * 40.0);
        wcol += step(0.965, h) * smoothstep(0.22, 0.06, dotd) * tw
            * biolum * vec3<f32>(0.15, 0.9, 0.8) * 0.8;
    }

    // ── Soft edge darkening, the style's own (the frame's vignette is
    // put back by `shown` as well).
    let vd = length((uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0));
    wcol *= 1.0 - 0.4 * smoothstep(0.45, 0.95, vd);

    // ── Hard white foam lip riding the line itself.
    let lip = under * (1.0 - smoothstep(0.005, 0.009, delta));
    var outc = mix(mapped, clamp(wcol, vec3<f32>(0.0), vec3<f32>(1.5)), under * mix_amt);
    outc = mix(outc, vec3<f32>(0.94, 0.98, 1.0), lip * mix_amt);
    return shown(clamp(outc, vec3<f32>(0.0), vec3<f32>(4.0)), uv);
}
