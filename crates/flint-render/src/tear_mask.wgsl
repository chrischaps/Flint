// The frame's tear (`[post_process] tear`, #746; render modes 1-4 and 6
// before it), shared by the blend draw, which shows a whole-frame style
// with `blend_source = "tear"` only inside it (stylize/blend.rs), and the
// style mask, which multiplies it into the blend of entities whose style
// follows it (#397). One copy, so the two can't drift: a styled prop turns
// exactly where the frame tears. Rust prepends this file to each
// (`TEAR_MASK_WGSL`); WGSL has no include.
//
// Noise ported from ocean_shader.wgsl: Hoskins-style sinless hash so
// large time-drifted coords don't degenerate. The composite's film grain,
// the reveal's edge and the reality styles (matrix, underwater) use these
// too.
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
// `tear` is `postprocess::tear_params`: x = the amount (`tear`), y = the
// scale (`tear_scale`), z = 1 for the iris (`tear_iris`), w = the post time.
// Patches (z < 0.5): drifting fbm patches whose coverage grows from
// scattered holes to full-screen as the amount rises — patch interiors are
// fully torn so the effect reads as reality tearing, not a crossfade.
// Iris: radial from screen center with an fbm-wobbled rim.
// A scale of 0 or less gets the default 3 (#395): at ~0 the noise is one
// value over the whole screen, so the sweep tears all or nothing.
fn tear_mask_at(uv: vec2<f32>, aspect: f32, tear: vec4<f32>) -> f32 {
    let m = tear.x;
    let t = tear.w;
    var scale = tear.y;
    if (scale <= 0.0) { scale = 3.0; }
    if (tear.z < 0.5) {
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
