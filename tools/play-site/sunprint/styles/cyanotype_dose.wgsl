// Cyanotype, pass 1: the dose, into the half-size `dose` target.
//
// The light the paper received is the picture's luminance. It creeps under
// an edge, so the dose here is a disc of 16 taps on a golden-angle spiral,
// up to 9 points across at bleed 1, measured in points so it reads the
// same on any display.

fn style(p: StylePixel) -> vec4<f32> {
    let radius = params.bleed * 9.0 * flint.pixels_per_point;
    var sum = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let f = f32(i);
        let r = radius * sqrt((f + 0.5) / 16.0);
        let a = f * 2.39996323;
        let offset = vec2<f32>(cos(a), sin(a)) * r * flint.texel;
        let c = clamp(flint_sample(p.uv + offset).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        sum = sum + flint_luma(c);
    }
    return vec4<f32>(sum / 16.0, 0.0, 0.0, 1.0);
}
