// Structure Tensor computation for Anisotropic Kuwahara filter
//
// Computes Sobel gradients of the HDR buffer luminance and outputs
// the structure tensor components (Jx*Jx, Jx*Jy, Jy*Jy, 0) per pixel.

struct KuwaharaTensorUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaTensorUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Fullscreen triangle
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;

    // Sample 3x3 neighborhood luminance
    let tl = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, -tx.y)).rgb);
    let tc = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(0.0, -tx.y)).rgb);
    let tr = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, -tx.y)).rgb);
    let ml = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, 0.0)).rgb);
    let mr = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, 0.0)).rgb);
    let bl = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, tx.y)).rgb);
    let bc = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(0.0, tx.y)).rgb);
    let br = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, tx.y)).rgb);

    // Sobel gradients
    let jx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    let jy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);

    // Structure tensor components: (E, F, G, 0) = (Jx*Jx, Jx*Jy, Jy*Jy, 0)
    return vec4<f32>(jx * jx, jx * jy, jy * jy, 0.0);
}
