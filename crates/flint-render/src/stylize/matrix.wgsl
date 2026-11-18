// Matrix (#746; render mode 1): the screen quantized into glyph cells,
// procedural dot-matrix glyphs lit by the picture's own luminance (foam
// crests read as bright code) with per-column falling rain heads.
//
// p0 = cell width and height in pixels, glyph rerolls a second, fall speed
// p1 = ink rgb, the rain head's glow
// p2 = ground rgb (an unlit cell)
// p3 = the rain head's rgb

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let frag_xy = in.position.xy;
    let cell_px = max(r.p0.xy, vec2<f32>(1.0));
    let cell = floor(frag_xy / cell_px);
    let cell_uv = fract(frag_xy / cell_px);

    // The picture's luminance at the cell's centre, before the vignette,
    // so brightness matches what the eye saw.
    let center_uv = (cell + 0.5) * cell_px * r.texel_size;
    let lum = flint_luma(picture(center_uv));

    // Glyph reroll: per-cell phase so the field flickers, not strobes.
    var rate = r.p0.z;
    if (rate < 0.01) { rate = 6.0; }
    let gid = floor(r.time * rate + hash2(cell) * 8.0);

    // Luminance contrast curve: a cel ocean is uniformly bright, so a
    // linear map saturates — square it and let glyph DENSITY carry the
    // structure too (dark cells go sparse, foam crests go dense).
    let li = clamp(lum * lum * 1.4, 0.0, 1.0);

    // 3x5 live dots inside a 4x6 grid — the dead row/column separates glyphs.
    let g = floor(cell_uv * vec2<f32>(4.0, 6.0));
    let f = fract(cell_uv * vec2<f32>(4.0, 6.0));
    var dot_on = step(mix(0.78, 0.38, li),
        hash2(cell * 17.0 + g + vec2<f32>(gid * 31.0, gid * 7.0)));
    dot_on *= step(g.x, 2.5) * step(g.y, 4.5);
    let inset = step(0.15, f.x) * step(f.x, 0.85) * step(0.1, f.y) * step(f.y, 0.9);
    let dots = dot_on * inset;

    // Falling rain head per column: bright leader, exponential trail above.
    let rows = 1.0 / (r.texel_size.y * cell_px.y);
    let speed = (0.25 + 0.75 * hash2(vec2<f32>(cell.x, 7.0))) * r.p0.w;
    let head_row = fract(r.time * speed * 0.12 + hash2(vec2<f32>(cell.x, 13.0)))
        * (rows + 20.0) - 10.0;
    let behind = head_row - cell.y; // >0 above/behind the falling head
    var trail = 0.12;
    if (behind >= 0.0) {
        trail = 0.12 + 0.88 * exp(-0.2 * behind);
    }
    let head_glow = smoothstep(1.5, 0.0, abs(behind));

    let intensity = clamp(li * 1.1 + 0.05, 0.0, 1.0) * trail * dots;
    var mcol = mix(r.p2.rgb, r.p1.rgb, intensity);
    mcol += r.p3.rgb * head_glow * dots * r.p1.w;
    return shown(mcol, in.uv);
}
