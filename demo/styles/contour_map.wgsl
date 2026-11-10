// Survey contours (#274): isolines of world height and range rings of
// view distance, a pixel wide wherever the field steps a whole band.

// 1 on a line of `field` every `step`, antialiased by its screen gradient.
fn isoline(field: f32, step: f32) -> f32 {
    let f = field / step;
    let w = max(fwidth(f), 1e-4);
    let d = abs(fract(f - 0.5) - 0.5) / w;
    return 1.0 - clamp(d - 0.5, 0.0, 1.0);
}

fn style(p: StylePixel) -> vec4<f32> {
    // fwidth needs uniform control flow: compute every field first
    let world = flint_world_pos(p.uv);
    // Range rings: distance from the eye, so they curve round it
    let range = length(world - flint.camera_pos);
    let h = isoline(world.y, params.spacing);
    let index = isoline(world.y, params.spacing * 5.0);
    let r = isoline(range, params.ring) * 0.45;
    let sky = flint_depth(p.uv) >= 0.99999;

    // The map's paper: the picture, washed toward cream
    let ink = vec3<f32>(params.ink_r, params.ink_g, params.ink_b);
    let paper = vec3<f32>(0.95, 0.92, 0.84);
    let tone = flint_luma(p.color.rgb);
    var c = mix(paper * (0.75 + 0.25 * tone), p.color.rgb, params.wash);

    // Every fifth height line is an index contour, drawn heavier; lines
    // thin out with distance so the far field stays legible
    let line = clamp(max(max(h * 0.7, index), r), 0.0, 1.0);
    let fade = 1.0 - smoothstep(30.0, 70.0, range);
    c = mix(c, ink, line * fade * select(1.0, 0.0, sky));
    return vec4<f32>(c, 1.0);
}
