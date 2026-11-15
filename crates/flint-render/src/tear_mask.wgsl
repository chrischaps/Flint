// The reality tear's mask (render modes 1-4 and 6), shared by the
// composite, which draws the tear's look through it, and the style mask,
// which multiplies it into the blend of entities whose style has
// `blend_source = "tear"` (#397). One copy, so the two can't drift: a
// styled prop turns exactly where the composite tears. Rust prepends this
// file to each (`TEAR_MASK_WGSL`); WGSL has no include.
//
// Noise ported from ocean_shader.wgsl: Hoskins-style sinless hash so
// large time-drifted coords don't degenerate. The composite's other modes
// (Matrix glyphs, underwater, grain) use these too.
fn hash2(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 3; i = i + 1) {
        v += a * value_noise(q);
        q = q * 2.13 + vec2<f32>(17.0, 9.2);
        a *= 0.5;
    }
    return v;
}

// Where the other reality shows through: 1 = torn, 0 = normal view, at
// screen `uv` (0..1, y down) of a frame `aspect` = width / height wide.
// `m` is the mode's mix, `mode_params` its tuning, `t` the post time.
// Style 0 (mode_params.y < 0.5): drifting fbm patches whose coverage grows
// from scattered holes to full-screen as m rises — patch interiors
// are fully torn so the effect reads as reality tearing, not a crossfade.
// Style 1: radial iris from screen center with an fbm-wobbled rim.
// A scale of 0 or less means "unset" and gets the default 3 (#395): at ~0 the
// noise is one value over the whole screen, so the sweep tears all or nothing.
// The config can't default it instead, since mode 5 reads x as eye depth.
fn tear_mask_at(
    uv: vec2<f32>,
    aspect: f32,
    m: f32,
    mode_params: vec4<f32>,
    t: f32,
) -> f32 {
    var scale = mode_params.x;
    if (scale <= 0.0) { scale = 3.0; }
    if (mode_params.y < 0.5) {
        let p = uv * vec2<f32>(aspect, 1.0) * scale
            + vec2<f32>(t * 0.02, -t * 0.013);
        // 3-octave fbm lives in ~[0, 0.875] with mean ≈ 0.44: sweep the
        // threshold across that span so coverage tracks m, and pin
        // the top so m = 1 is guaranteed fully torn.
        let th = 0.44 + 0.4 * (1.0 - 2.0 * m);
        let patches = smoothstep(th - 0.08, th + 0.08, fbm(p));
        return max(patches, smoothstep(0.85, 1.0, m));
    }
    let d = length((uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0));
    let dir = normalize(uv - vec2<f32>(0.5, 0.5) + vec2<f32>(0.0001, 0.0));
    let wobble = (fbm(dir * 3.0 + vec2<f32>(t * 0.1, 0.0)) - 0.5) * 0.1;
    let r0 = m * (0.5 * aspect + 0.65) + wobble;
    return smoothstep(r0, r0 - 0.15, d);
}
