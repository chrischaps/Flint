// A test fixture (#417): this style must fail to compile. Line 9 gives a
// vec3 the picture's vec4, the kind of slip a hand-written style makes.
// The goldens check that the frame draws as if the style weren't there and
// that the error points here, at broken.wgsl:9.
//
// Don't fix it.

fn style(p: StylePixel) -> vec4<f32> {
    let warm: vec3<f32> = p.color;
    return vec4<f32>(mix(p.color.rgb, warm, params.amount), p.color.a);
}
