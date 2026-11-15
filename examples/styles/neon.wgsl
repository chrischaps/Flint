// Neon isolines (#300): an hdr style that dims the scene and traces world
// height in lines brighter than white, so bloom (fed by this style) glows
// from the lines themselves.

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
    let line = isoline(world.y, params.spacing);
    let range = length(world - flint.camera_pos);
    let sky = flint_depth(p.uv) >= 0.99999;

    // The ink is a display colour; the hdr stage is linear light
    let ink = pow(vec3<f32>(params.ink_r, params.ink_g, params.ink_b), vec3<f32>(2.2));
    // Lines thin out with distance so the far field doesn't turn to haze
    let fade = (1.0 - smoothstep(25.0, 60.0, range)) * select(1.0, 0.0, sky);
    let c = p.color.rgb * params.dim + ink * params.glow * line * fade;
    return vec4<f32>(c, p.color.a);
}
