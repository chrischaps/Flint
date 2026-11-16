// A style with `blend_source = "tear"` (#746): what the style read, drawn
// back over what it wrote everywhere the frame isn't torn, so the style
// shows only inside the tear's holes, `blend` of it there. The mask is
// tear_mask.wgsl's (prepended), the one the style mask multiplies into a
// torn prop's blend (#397), so a whole-frame style and a prop tear together.
// The pipeline's blend does the mixing: the alpha written is how much of
// the input goes back.

struct TearBlend {
    // postprocess::tear_params: amount, scale, iris, post time
    tear: vec4<f32>,
    // The style's blend, the frame's width / height, unused
    blend: f32,
    aspect: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(1) @binding(0) var<uniform> u: TearBlend;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let shown = u.blend * tear_mask_at(in.uv, u.aspect, u.tear);
    let read = textureSample(input_texture, input_sampler, in.uv);
    return vec4<f32>(read.rgb, 1.0 - shown);
}
