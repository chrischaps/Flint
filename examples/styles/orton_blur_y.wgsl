// Orton, pass 3: the same Gaussian down `wide`, into `soft`.

fn style(p: StylePixel) -> vec4<f32> {
    let px = params.radius * flint.pixels_per_point * flint.pass_size.y / flint.resolution.y;
    let step = vec2<f32>(0.0, px / 6.0 / flint.pass_size.y);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = -6; i <= 6; i++) {
        let y = f32(i) / 3.0;
        let w = exp(-0.5 * y * y);
        sum += flint_target_wide(p.uv + step * f32(i)).rgb * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, 1.0);
}
