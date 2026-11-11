// Orton, pass 4: the sandwich. The soft copy is screened over the sharp
// frame (light adds to light, nothing darkens), then the two are mixed by
// `amount`. The last pass writes the stage's output, in display values.

fn style(p: StylePixel) -> vec4<f32> {
    let sharp = p.color.rgb;
    // The target is filtered: reading it at full size upsamples it smoothly
    let soft = clamp(flint_target_soft(p.uv).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let screen = vec3<f32>(1.0) - (vec3<f32>(1.0) - sharp) * (vec3<f32>(1.0) - soft);
    return vec4<f32>(mix(sharp, screen, params.amount), p.color.a);
}
