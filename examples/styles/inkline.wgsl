// Ink line (#301): flat paper tones under ink drawn from the shared
// geometry edges. flint_edges(uv) is (silhouette, crease, direction):
// strengths of 1 are edges at the scene's edge_depth / edge_normal.

// How much ink one edge pixel wants: a silhouette in full, a crease lighter
fn ink_at(uv: vec2<f32>) -> f32 {
    let e = flint_edges(uv) * params.reach;
    return max(smoothstep(0.7, 1.0, e.r), smoothstep(0.7, 1.0, e.g) * params.crease);
}

fn style(p: StylePixel) -> vec4<f32> {
    let paper = vec3<f32>(0.96, 0.93, 0.86);
    let ink = vec3<f32>(0.10, 0.09, 0.12);

    // A few flat tones of the picture's light, tinted by a little of its
    // colour, so form reads without competing with the line
    let tone = flint_luma(p.color.rgb);
    let steps = max(params.tones, 2.0);
    let flat = floor(tone * steps + 0.5) / steps;
    let tint = mix(vec3<f32>(1.0), p.color.rgb / max(tone, 1e-3), params.wash);
    var c = paper * mix(0.62, 1.0, flat) * clamp(tint, vec3<f32>(0.6), vec3<f32>(1.2));

    // Ink: a silhouette in full, a crease lighter. Reading the four
    // neighbours as well gives the line a little body and closes the
    // gaps a one-pixel staircase leaves on a slanted outline
    let t = flint.texel;
    let centre = ink_at(p.uv);
    let around = max(
        max(ink_at(p.uv + vec2<f32>(t.x, 0.0)), ink_at(p.uv - vec2<f32>(t.x, 0.0))),
        max(ink_at(p.uv + vec2<f32>(0.0, t.y)), ink_at(p.uv - vec2<f32>(0.0, t.y)))
    );
    let line = max(centre, around * 0.55);
    c = mix(c, ink, line);
    return vec4<f32>(c, 1.0);
}
