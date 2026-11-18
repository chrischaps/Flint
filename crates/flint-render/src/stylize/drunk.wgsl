// Drunk (#746; render mode 3): the frame sways and sees double. The UV
// every sample uses wanders, so the whole picture moves together, and a
// ghost of it drifts sideways over it. The style draws its own blend: the
// sway and the ghost grow with it, which a mix of the input back can't do.
//
// p0 = sway, ghost, speed, the blend

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let m = clamp(r.p0.w, 0.0, 1.0);
    let t = r.time * r.p0.z;
    let amp = 0.006 * m * r.p0.x;
    let uv = in.uv + vec2<f32>(
        sin(t * 0.7 + in.uv.y * 3.0),
        cos(t * 0.9 + in.uv.x * 2.5) * 0.6
    ) * amp;
    var color = sample_linear(uv);
    let ghost_off = vec2<f32>(0.012 + 0.010 * sin(t * 0.5), 0.0) * m * r.p0.y;
    let ghost = sample_linear(uv + ghost_off);
    color = mix(color, ghost, clamp(0.35 * m * r.p0.y, 0.0, 1.0));
    return vec4<f32>(select(flint_to_display(color), color, r.srgb > 0.5), 1.0);
}
