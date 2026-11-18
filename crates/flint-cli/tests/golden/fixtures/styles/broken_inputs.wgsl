// A test fixture (#563): this style reads depth, the mask and the edges,
// and must fail to compile. Line 11 takes the edges texel (a vec4) as an
// f32, the kind of slip a hand-written style makes. The goldens check that
// the frame draws as if the style weren't there (and that the edges, which
// another style reads, are untouched) and that the error points here, at
// broken_inputs.wgsl:11.
//
// Don't fix it.

fn style(p: StylePixel) -> vec4<f32> {
    let edge: f32 = flint_edges(p.uv);
    let near = 1.0 - saturate(flint_view_depth(p.uv) / 40.0);
    let ink = edge * near * params.ink * flint_style_blend(p.uv);
    return vec4<f32>(mix(p.color.rgb, vec3<f32>(0.05), ink), p.color.a);
}
