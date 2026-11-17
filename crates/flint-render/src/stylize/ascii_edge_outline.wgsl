// ASCII edges (#267), pass 1 of 3 since #743: the style mask's outlines.
//
// The depth steps and creases are the shared edge pass's (`edges.wgsl`);
// this pass adds what only ASCII knows, where one of its entities meets
// anything else (ascii_edge_source.wgsl's `outline_pixel`), one fragment
// a depth pixel. Drawn only while the mask is bound. Output: an edge pixel
// (the outline's structure tensor, w = 3) where there is an outline, else
// 0; the cell pass reads it before the shared edges.

struct OutlineUniforms {
    // The edge cell pass's uniform starts with the same `src`, and its
    // buffer is what this pass binds
    src: EdgeSource,
};

@group(0) @binding(0) var<uniform> u: OutlineUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return outline_pixel(vec2<i32>(in.position.xy));
}
