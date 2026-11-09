// The framework's cross-fade (#396). Drawn over a style's output right
// after its pass, blended: on every pixel whose style mask id belongs to
// the style that just drew (the instance index says which) and whose blend
// (mask .g, #278; less where a veil is in front, #290) is below 1, the
// style's input comes back by 1 - blend. Elsewhere the fragment is
// discarded and the style's output stays as it drew it.
//
// The input is the output's size (the chain's ping-pong targets), read
// texel for texel. The mask is the scene's size, smaller than the output
// at the display stage under a render scale, so it is read by position
// scaled the way ASCII reads it.

// The registry index + 1 of the style each mask id belongs to; 0 = none
// (unstyled, an opt-out, or a style that isn't registered)
@group(0) @binding(0) var<storage, read> owners: array<u32>;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(2) @binding(0) var style_mask: texture_2d<u32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The style that just drew, as its registry index + 1
    @location(0) @interpolate(flat) me: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.me = ii;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let size = vec2<i32>(textureDimensions(input_texture));
    let msize = vec2<i32>(textureDimensions(style_mask));
    let mp = vec2<i32>(floor(p * vec2<f32>(msize) / vec2<f32>(size)));
    let m = textureLoad(style_mask, clamp(mp, vec2<i32>(0), msize - 1), 0).rg;
    if m.x == 0u || m.x >= arrayLength(&owners) || owners[m.x] != in.me {
        discard;
    }
    let blend = f32(m.y) / 65535.0;
    if blend >= 1.0 {
        discard;
    }
    let src = textureLoad(input_texture, clamp(vec2<i32>(floor(p)), vec2<i32>(0), size - 1), 0);
    // Blended as src * a + dst * (1 - a): the style's output keeps `blend`
    return vec4<f32>(src.rgb, 1.0 - blend);
}
