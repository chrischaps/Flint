// A test fixture (#563): this HDR-stage style must fail to compile. Line
// 10 calls `flint_glow`, which no prelude defines (the kind of slip a
// hand-written style makes, half-remembering a helper's name). The goldens
// check that the frame, bloom included, draws as if the style weren't
// there and that the error points here, at broken_hdr.wgsl:10.
//
// Don't fix it.

fn style(p: StylePixel) -> vec4<f32> {
    let glow = flint_glow(p.color.rgb) * params.gain;
    return vec4<f32>(p.color.rgb + glow, p.color.a);
}
