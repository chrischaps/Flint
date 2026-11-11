// Risograph (#274): the user's whole style. `p.color` is what the eye
// sees; `params.*` are the .shader.toml's [params]; `flint.*` the frame.

// Coverage of a dot screen at angle `a`: 1 inside a dot sized for `tone`.
fn screen(pos: vec2<f32>, a: f32, pitch: f32, tone: f32) -> f32 {
    let r = mat2x2<f32>(cos(a), -sin(a), sin(a), cos(a));
    let q = r * pos / pitch;
    let cell = fract(q) - 0.5;
    let radius = sqrt(clamp(tone, 0.0, 1.0)) * 0.62;
    let d = length(cell);
    return 1.0 - smoothstep(radius - 0.06, radius + 0.06, d);
}

fn style(p: StylePixel) -> vec4<f32> {
    let ppp = flint.pixels_per_point;
    let pitch = params.dot * ppp;
    let ink_a = vec3<f32>(params.ink_a_r, params.ink_a_g, params.ink_a_b);
    let ink_b = vec3<f32>(params.ink_b_r, params.ink_b_g, params.ink_b_b);
    let paper = vec3<f32>(params.paper_r, params.paper_g, params.paper_b);

    // Drum A takes the warm half of the picture, drum B the shadows; B is
    // printed `shift` points off register.
    let offset = vec2<f32>(params.shift, -params.shift * 0.6) * ppp;
    let here = p.color.rgb;
    let there = flint_sample(p.uv + offset * flint.texel).rgb;
    // Highlights stay paper: both drums fade out above a luma of ~0.7
    let l_here = flint_luma(here);
    let l_there = flint_luma(there);
    let warmth = clamp((here.r - here.b) * 2.0, 0.0, 1.0);
    let warm = clamp((1.0 - l_here) * 0.3 + warmth * 0.35, 0.0, 0.6)
        * (1.0 - smoothstep(0.55, 0.85, l_here));
    let dark = pow(clamp(1.0 - l_there * 2.2, 0.0, 1.0), 1.6);

    let a = screen(p.pos, 0.26, pitch, warm);
    let b = screen(p.pos + offset, 1.31, pitch, dark);

    // Inks multiply on paper, with a little grain in each drum
    let grain = flint_hash(floor(p.pos / ppp)) * 0.12;
    var c = paper * (1.0 - 0.04 * flint_hash(floor(p.pos / (ppp * 3.0))));
    c *= mix(vec3<f32>(1.0), ink_a, a * (0.92 - grain));
    c *= mix(vec3<f32>(1.0), ink_b, b * (0.95 - grain));
    return vec4<f32>(c, 1.0);
}
