// Style ownership (#384): after a style draws, the pixels another style
// owns get the style's input back, so each pixel is one style's. A pixel
// is owned by the style its mask id's variant names (`owner`, the
// registry index; NONE for unstyled ids and opt-outs); the style that just
// drew is `instance_index`. Its own pixels and unowned ones keep what it
// drew (discarded); a foreign pixel takes the input, `blend` of the way
// (the mask's .g), through the pipeline's alpha blend, so a foreign entity
// fading out of its style (blend 0) is this style's again.

struct Uniforms {
    // owner[id] for id 0..255, four to a vec4
    owner: array<vec4<u32>, 64>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var input_tex: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;
@group(2) @binding(0) var style_mask: texture_2d<u32>;

const NONE: u32 = 0xffffffffu;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) style: u32,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) style: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.style = style;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(input_tex));
    let msize = vec2<i32>(textureDimensions(style_mask));
    let px = vec2<i32>(in.position.xy);
    // The mask is the scene's size, smaller than the output at the display
    // stage under a render scale: read it by UV
    let uv = in.position.xy / size;
    let mp = clamp(vec2<i32>(uv * vec2<f32>(msize)), vec2<i32>(0), msize - 1);
    let m = textureLoad(style_mask, mp, 0);
    let id = m.r;
    if id == 0u || id >= 256u {
        discard;
    }
    let owner = u.owner[id / 4u][id % 4u];
    if owner == NONE || owner == in.style {
        discard;
    }
    let blend = f32(m.g) / 65535.0;
    if blend <= 0.0 {
        discard;
    }
    let c = textureLoad(input_tex, px, 0);
    return vec4<f32>(c.rgb, blend);
}
