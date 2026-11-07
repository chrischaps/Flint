// Display bloom (#321): a bloom over the finished, displayed picture, for
// the styles that draw after the HDR bloom has run (ASCII's glyphs,
// pencil's highlights). Four entry points share one uniform shape:
//
//   fs_threshold  input (output size)  -> mip 0 (half size): the bright
//                 part of each pixel, judged by its display value
//   fs_down       mip i-1 -> mip i: 13-tap filter (Jimenez 2014)
//   fs_up         mip i+1 -> mip i, added: 9-tap tent
//   fs_composite  input + mip 0 * intensity -> output
//
// The chain holds linear light in Rgba16Float. On an sRGB target the
// input samples come back linear and the composite's write is encoded, so
// the halo adds as light does; on a plain target both are display values.

struct BloomUniforms {
    // The source's texel size
    texel: vec2<f32>,
    threshold: f32,
    knee: f32,
    intensity: f32,
    srgb: f32,
    saturation: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> params: BloomUniforms;
@group(1) @binding(0) var src_texture: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;
// The composite's second texture: the bloom (mip 0)
@group(2) @binding(0) var bloom_texture: texture_2d<f32>;
@group(2) @binding(1) var bloom_sampler: sampler;

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
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn sample_src(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(src_texture, src_sampler, uv).rgb;
}

// How much of a pixel passes: a soft knee around the threshold, on the
// brightest channel of what the viewer sees
fn pass_weight(display_peak: f32) -> f32 {
    let knee = max(params.threshold * params.knee, 1e-4);
    let soft = clamp(display_peak - params.threshold + knee, 0.0, 2.0 * knee);
    let curve = soft * soft / (4.0 * knee);
    return max(curve, display_peak - params.threshold) / max(display_peak, 1e-4);
}

@fragment
fn fs_threshold(in: VertexOutput) -> @location(0) vec4<f32> {
    // Mip 0 is half the input's size: four bilinear taps at the corners of
    // this texel's 2x2 footprint read a 4x4 box, so a one-pixel glyph
    // stroke lands whole wherever it falls
    let ts = params.texel;
    var color = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let o = vec2<f32>(f32(i & 1u) * 2.0 - 1.0, f32(i >> 1u) * 2.0 - 1.0);
        let c = clamp(sample_src(in.uv + o * ts), vec3<f32>(0.0), vec3<f32>(1.0));
        let shown = select(c, to_display(c), params.srgb > 0.5);
        let peak = max(shown.r, max(shown.g, shown.b));
        color += c * pass_weight(peak);
    }
    color *= 0.25;
    // Saturation of the halo: 1 keeps the glyph's hue, 0 glows white
    let grey = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(vec3<f32>(grey), color, params.saturation);
    return vec4<f32>(max(color, vec3<f32>(0.0)), 1.0);
}

@fragment
fn fs_down(in: VertexOutput) -> @location(0) vec4<f32> {
    let ts = params.texel;
    let a = sample_src(in.uv);
    let b = sample_src(in.uv + vec2<f32>(-0.5, -0.5) * ts);
    let c = sample_src(in.uv + vec2<f32>( 0.5, -0.5) * ts);
    let d = sample_src(in.uv + vec2<f32>(-0.5,  0.5) * ts);
    let e = sample_src(in.uv + vec2<f32>( 0.5,  0.5) * ts);
    let f = sample_src(in.uv + vec2<f32>(-1.0, -1.0) * ts);
    let g = sample_src(in.uv + vec2<f32>( 0.0, -1.0) * ts);
    let h = sample_src(in.uv + vec2<f32>( 1.0, -1.0) * ts);
    let i = sample_src(in.uv + vec2<f32>(-1.0,  0.0) * ts);
    let j = sample_src(in.uv + vec2<f32>( 1.0,  0.0) * ts);
    let k = sample_src(in.uv + vec2<f32>(-1.0,  1.0) * ts);
    let l = sample_src(in.uv + vec2<f32>( 0.0,  1.0) * ts);
    let m = sample_src(in.uv + vec2<f32>( 1.0,  1.0) * ts);
    var color = a * 0.125;
    color += (b + c + d + e) * 0.125;
    color += (f + h + k + m) * 0.03125;
    color += (g + i + j + l) * 0.0625;
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_up(in: VertexOutput) -> @location(0) vec4<f32> {
    let ts = params.texel;
    var color = sample_src(in.uv) * 4.0;
    color += (sample_src(in.uv + vec2<f32>(0.0, -1.0) * ts)
        + sample_src(in.uv + vec2<f32>(-1.0, 0.0) * ts)
        + sample_src(in.uv + vec2<f32>(1.0, 0.0) * ts)
        + sample_src(in.uv + vec2<f32>(0.0, 1.0) * ts)) * 2.0;
    color += sample_src(in.uv + vec2<f32>(-1.0, -1.0) * ts)
        + sample_src(in.uv + vec2<f32>(1.0, -1.0) * ts)
        + sample_src(in.uv + vec2<f32>(-1.0, 1.0) * ts)
        + sample_src(in.uv + vec2<f32>(1.0, 1.0) * ts);
    // Each level is added onto the one above (blend One + One): the
    // narrow levels keep the glyph's shape, the wide ones join a run
    return vec4<f32>(color / 16.0, 1.0);
}

@fragment
fn fs_composite(in: VertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(src_texture, src_sampler, in.uv);
    let halo = textureSample(bloom_texture, bloom_sampler, in.uv).rgb;
    return vec4<f32>(src.rgb + halo * params.intensity, src.a);
}
