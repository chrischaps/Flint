// Darkroom print (#413): a picture graded through a LUT and printed on
// paper. `flint_lut_grade` and `flint_image_tiled_paper` are generated from
// the .shader.toml's image options; `params.finish` is an index with a
// constant for each of its choices.

fn style(p: StylePixel) -> vec4<f32> {
    let ppp = flint.pixels_per_point;

    // The grade: a 3D LUT read from its strip, mixed in
    let graded = mix(p.color.rgb, flint_lut_grade(p.color.rgb), params.grade_mix);

    // The paper, repeated at its own size in points
    let paper_uv = p.pos / (flint_image_size_paper() * params.paper_scale * ppp);
    let paper = mix(vec3<f32>(1.0), flint_image_tiled_paper(paper_uv).rgb, params.paper_strength);
    // Its tooth: brighter fibres take less ink
    let tooth = flint_luma(paper) - flint_luma(flint_image_tiled_paper(paper_uv + vec2<f32>(0.37, 0.61)).rgb);

    var ink = graded;
    if (params.finish == FLINT_FINISH_MATTE) {
        // Matte: the blacks lift into the paper, the ink sits in the tooth
        ink = mix(vec3<f32>(0.075), vec3<f32>(1.0), ink) * (1.0 + tooth * 0.25);
    } else {
        // Gloss: deep blacks, a little less of the paper through the ink
        ink = pow(ink, vec3<f32>(1.08)) * (1.0 + tooth * 0.2);
    }
    var c = ink * paper;

    // The print's border: paper the enlarger never reached, with a soft
    // edge to the exposure
    let margin = 18.0 * ppp;
    let edge = min(min(p.pos.x, flint.resolution.x - p.pos.x), min(p.pos.y, flint.resolution.y - p.pos.y));
    let exposed = smoothstep(margin * 0.6, margin, edge + tooth * margin * 0.25);
    c = mix(mix(c, paper, params.border), c, exposed);
    return vec4<f32>(c, 1.0);
}
