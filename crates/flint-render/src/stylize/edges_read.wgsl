// Reading the shared edges (#743) in the scene's own measures, for a
// built-in style that sets its own level instead of the shared threshold.
// `e` is a texel of the edge texture (`edges.wgsl`), `edge_depth` and
// `edge_normal` the shared thresholds it was drawn with.

// The depth step at an edge pixel, as a share of its view distance: across
// a step of `s` the relative-distance Sobel is about 4 s, a strength of 1
// at `edge_depth`
fn flint_edge_step(e: vec4<f32>, edge_depth: f32) -> f32 {
    return e.r * edge_depth * 0.25;
}

// The crease's turn, 1 - cos of the angle between its two faces: across a
// fold of angle a the one-sided normals differ by 2 sin(a / 2), a Sobel of
// 8 sin(a / 2), a strength of 1 at `edge_normal`
fn flint_edge_turn(e: vec4<f32>, edge_normal: f32) -> f32 {
    let half_sine = min(e.g * edge_normal * 0.125, 1.0);
    return 2.0 * half_sine * half_sine;
}

// The edge texel under `uv` (the edges are the depth's size)
fn flint_edge_texel(edges: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(edges));
    return textureLoad(edges, clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1), 0);
}
