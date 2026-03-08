// Structure Tensor Gaussian Blur for Anisotropic Kuwahara filter
//
// 5x5 Gaussian blur on the structure tensor texture.
// NOT depth-aware — orientations should bleed smoothly for coherent brush strokes.

struct KuwaharaTensorBlurUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaTensorBlurUniforms;

@group(1) @binding(0)
var tensor_texture: texture_2d<f32>;
@group(1) @binding(1)
var tensor_sampler: sampler;

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
    let tx = params.texel_size;

    // 5x5 Gaussian kernel (sigma ~1.0, separable product normalized)
    let w = array<f32, 5>(1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0);

    var result = vec4<f32>(0.0);
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
            let weight = w[x + 2] * w[y + 2];
            result += textureSample(tensor_texture, tensor_sampler, in.uv + offset) * weight;
        }
    }

    return result;
}
