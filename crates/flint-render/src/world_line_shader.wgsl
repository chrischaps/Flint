// Depth-tested world-space lines (ADR 0083). Each segment arrives as a quad
// already in NDC (w = 1), built on the CPU by world_line_pipeline.rs. The
// fragment fades the quad's edges over a pixel, which anti-aliases it.

struct VsIn {
    @location(0) position: vec4<f32>,
    @location(1) color: vec4<f32>,
    // x: signed distance from the centre line in pixels; y: half width.
    @location(2) edge: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) edge: vec2<f32>,
};

@vertex
fn vs_world_line(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = v.position;
    out.color = v.color;
    out.edge = v.edge;
    return out;
}

@fragment
fn fs_world_line(v: VsOut) -> @location(0) vec4<f32> {
    // Full inside the line, a one-pixel ramp at its edge. A line under a
    // pixel wide never reaches full cover, so it fades rather than flickers.
    let cover = clamp(v.edge.y + 0.5 - abs(v.edge.x), 0.0, 1.0);
    return vec4<f32>(v.color.rgb, v.color.a * cover);
}
