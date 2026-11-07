// Watercolor wash (#272): one direction of a separable Gaussian, sigma
// half the wash radius, 17 linear taps reaching two radii either way.
// Run twice (horizontal, then vertical) it is how far pigment diffuses.

struct WashUniforms {
    texel: vec2<f32>,
    dir: vec2<f32>,
    radius: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> params: WashUniforms;
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (params.radius < 0.5) {
        return textureSampleLevel(input_texture, input_sampler, in.uv, 0.0);
    }
    let step = params.dir * params.texel * (params.radius / 4.0);
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var k = -8; k <= 8; k++) {
        // t in sigmas: k / 4 radii = k / 2 sigmas
        let t = f32(k) * 0.5;
        let w = exp(-0.5 * t * t);
        sum += w * textureSampleLevel(input_texture, input_sampler, in.uv + step * f32(k), 0.0);
        weight += w;
    }
    return sum / weight;
}
