// Structure Tensor Gaussian Blur for Anisotropic Kuwahara filter
//
// 5x5 Gaussian blur on the structure tensor texture (fully unrolled).
// NOT depth-aware — orientations should bleed smoothly for coherent brush strokes.
//
// Kernel: outer product of [1,4,6,4,1]/16, giving weights /256.
// Unrolled to avoid dynamic array indexing that crashes some Vulkan shader compilers.

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

// Helper: sample tensor at pixel offset
fn tap(uv: vec2<f32>, ox: f32, oy: f32) -> vec4<f32> {
    let tx = params.texel_size;
    return textureSampleLevel(tensor_texture, tensor_sampler, uv + vec2<f32>(ox * tx.x, oy * tx.y), 0.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // 5x5 Gaussian blur — separable kernel [1, 4, 6, 4, 1] / 16
    // Full 2D weights = outer product / 256
    // Row -2 (col weights: 1, 4, 6, 4, 1)
    var r  = tap(uv, -2.0, -2.0) * (1.0 / 256.0);
    r     += tap(uv, -1.0, -2.0) * (4.0 / 256.0);
    r     += tap(uv,  0.0, -2.0) * (6.0 / 256.0);
    r     += tap(uv,  1.0, -2.0) * (4.0 / 256.0);
    r     += tap(uv,  2.0, -2.0) * (1.0 / 256.0);
    // Row -1
    r     += tap(uv, -2.0, -1.0) * (4.0 / 256.0);
    r     += tap(uv, -1.0, -1.0) * (16.0 / 256.0);
    r     += tap(uv,  0.0, -1.0) * (24.0 / 256.0);
    r     += tap(uv,  1.0, -1.0) * (16.0 / 256.0);
    r     += tap(uv,  2.0, -1.0) * (4.0 / 256.0);
    // Row 0 (center)
    r     += tap(uv, -2.0,  0.0) * (6.0 / 256.0);
    r     += tap(uv, -1.0,  0.0) * (24.0 / 256.0);
    r     += tap(uv,  0.0,  0.0) * (36.0 / 256.0);
    r     += tap(uv,  1.0,  0.0) * (24.0 / 256.0);
    r     += tap(uv,  2.0,  0.0) * (6.0 / 256.0);
    // Row +1
    r     += tap(uv, -2.0,  1.0) * (4.0 / 256.0);
    r     += tap(uv, -1.0,  1.0) * (16.0 / 256.0);
    r     += tap(uv,  0.0,  1.0) * (24.0 / 256.0);
    r     += tap(uv,  1.0,  1.0) * (16.0 / 256.0);
    r     += tap(uv,  2.0,  1.0) * (4.0 / 256.0);
    // Row +2
    r     += tap(uv, -2.0,  2.0) * (1.0 / 256.0);
    r     += tap(uv, -1.0,  2.0) * (4.0 / 256.0);
    r     += tap(uv,  0.0,  2.0) * (6.0 / 256.0);
    r     += tap(uv,  1.0,  2.0) * (4.0 / 256.0);
    r     += tap(uv,  2.0,  2.0) * (1.0 / 256.0);

    return r;
}
