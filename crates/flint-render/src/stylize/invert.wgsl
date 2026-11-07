// Invert (#262): the template style. Reads the finished picture and writes
// its negative, `strength` of the way. On an sRGB target the sample is
// linear, so it is encoded first and decoded after, to invert what the
// viewer sees rather than linear light.

struct InvertUniforms {
    strength: f32,
    srgb: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: InvertUniforms;
@group(1) @binding(0) var input_texture: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;

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

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(input_texture, input_sampler, in.uv);
    let rgb = clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let shown = select(rgb, to_display(rgb), params.srgb > 0.5);
    let inverted = mix(shown, vec3<f32>(1.0) - shown, params.strength);
    let out = select(inverted, to_linear(inverted), params.srgb > 0.5);
    return vec4<f32>(out, src.a);
}
