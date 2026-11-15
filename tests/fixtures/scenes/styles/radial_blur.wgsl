// A radial blur (#411): the frame streaks out from a point, sharp near it.
// The loop runs `samples` times; Flint guards it like every user loop, so
// even a bound that never ends stops after 4096 turns.

fn style(p: StylePixel) -> vec4<f32> {
    let center = vec2<f32>(params.center_x, params.center_y);
    let to_center = center - p.uv;
    // Sharp inside `clear`, the full streak a little past it
    let reach = smoothstep(params.clear, params.clear + 0.35, length(to_center));
    let step = to_center * params.strength * reach;
    let n = max(i32(params.samples), 1);
    var acc = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < n; i++) {
        let t = f32(i) / f32(n);
        // Nearer reads count more, so the streak fades along its length
        let w = 1.0 - 0.6 * t;
        acc += flint_sample(p.uv + step * t).rgb * w;
        weight += w;
    }
    return vec4<f32>(acc / weight, p.color.a);
}
