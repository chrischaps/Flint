// Orton, pass 1: the soft copy's exposure. Writes `exposed` at half size;
// a target holds what this returns, unchanged (display values here, since
// p.color is, and room above 1 because the target is rgba16f).

fn style(p: StylePixel) -> vec4<f32> {
    let c = p.color.rgb;
    let grey = vec3<f32>(flint_luma(c));
    let rich = max(mix(grey, c, params.saturation), vec3<f32>(0.0));
    return vec4<f32>(rich * params.exposure, 1.0);
}
