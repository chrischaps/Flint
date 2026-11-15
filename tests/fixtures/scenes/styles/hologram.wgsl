// Hologram (#274): only the pixels of entities whose style_mask names this
// style. `flint_style_blend` is 0 off them, their blend (0..1) on them.

fn covered(uv: vec2<f32>) -> f32 {
    return select(0.0, 1.0, flint_style_blend(uv) > 0.0);
}

fn style(p: StylePixel) -> vec4<f32> {
    let on = flint_style_blend(p.uv);
    if (on <= 0.0) {
        return p.color;
    }
    let ppp = flint.pixels_per_point;
    let cyan = vec3<f32>(0.35, 0.95, 1.0);

    // What's behind a hologram isn't drawn, so fake its transparency with
    // the picture's own luma pushed into cyan
    let l = flint_luma(p.color.rgb);
    var c = cyan * (0.2 + 1.1 * l);

    // Scanlines that crawl upward, and a slow band of interference
    let row = (p.pos.y / ppp + flint.time * 12.0) / params.lines;
    c *= 0.6 + 0.4 * step(0.5, fract(row));
    let band = smoothstep(0.0, 0.08, abs(fract(p.uv.y * 0.7 - flint.time * 0.21) - 0.5));
    c *= 0.8 + 0.2 * band;

    // A rim where the projection ends: neighbours a couple of points away
    let o = 2.0 * ppp * flint.texel;
    let inside = covered(p.uv + vec2<f32>(o.x, 0.0)) + covered(p.uv - vec2<f32>(o.x, 0.0))
        + covered(p.uv + vec2<f32>(0.0, o.y)) + covered(p.uv - vec2<f32>(0.0, o.y));
    c += cyan * clamp(4.0 - inside, 0.0, 1.0) * params.glow;

    // Flicker: a few frames a second go dim, chosen by a hash of time
    let f = flint_hash(vec2<f32>(floor(flint.time * 9.0), 3.0));
    c *= 1.0 - params.flicker * step(0.8, f);

    return vec4<f32>(mix(p.color.rgb, c, on * 0.9), 1.0);
}
