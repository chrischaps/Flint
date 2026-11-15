// Orton, pass 2: a 13-tap Gaussian across `exposed`, into `wide`.

fn style(p: StylePixel) -> vec4<f32> {
    // The radius is in points of the finished frame; this target is
    // pass_size wide, so a point is pass_size / resolution of its pixels
    let px = params.radius * flint.pixels_per_point * flint.pass_size.x / flint.resolution.x;
    let step = vec2<f32>(px / 6.0 / flint.pass_size.x, 0.0);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = -6; i <= 6; i++) {
        let x = f32(i) / 3.0;
        let w = exp(-0.5 * x * x);
        sum += flint_target_exposed(p.uv + step * f32(i)).rgb * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, 1.0);
}
