// Screen-Space Ambient Occlusion (SSAO) generation shader
//
// Samples the depth buffer and reconstructs view-space positions/normals
// to estimate ambient occlusion at each pixel. Uses a hemisphere kernel
// of 64 samples rotated by a 4x4 noise texture to break banding.

struct SsaoUniforms {
    inv_projection: mat4x4<f32>,
    projection: mat4x4<f32>,
    kernel: array<vec4<f32>, 64>,
    noise_scale: vec2<f32>,
    radius: f32,
    bias: f32,
    intensity: f32,
    near: f32,
    far: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> params: SsaoUniforms;

@group(1) @binding(0)
var depth_texture: texture_2d<f32>;
@group(1) @binding(1)
var depth_sampler: sampler;

@group(2) @binding(0)
var noise_texture: texture_2d<f32>;
@group(2) @binding(1)
var noise_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_ssao(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);

    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Linearize depth from the non-linear depth buffer value
fn linearize_depth(d: f32) -> f32 {
    return (params.near * params.far) / (params.far - d * (params.far - params.near));
}

// Reconstruct view-space position from UV and depth
fn view_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    // Map UV to clip space: [0,1] → [-1,1], flip Y
    let clip = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let view_pos = params.inv_projection * clip;
    return view_pos.xyz / view_pos.w;
}

@fragment
fn fs_ssao(in: VsOut) -> @location(0) f32 {
    let uv = in.uv;
    let dims = vec2<f32>(textureDimensions(depth_texture));
    let texel = vec2<f32>(1.0 / dims.x, 1.0 / dims.y);

    let depth = textureSample(depth_texture, depth_sampler, uv).r;

    // Skip sky pixels (depth at or near 1.0)
    if (depth >= 0.9999) {
        return 1.0;
    }

    let frag_pos = view_pos_from_depth(uv, depth);

    // Reconstruct normal from depth via finite differences
    let depth_r = textureSample(depth_texture, depth_sampler, uv + vec2<f32>(texel.x, 0.0)).r;
    let depth_l = textureSample(depth_texture, depth_sampler, uv - vec2<f32>(texel.x, 0.0)).r;
    let depth_t = textureSample(depth_texture, depth_sampler, uv + vec2<f32>(0.0, texel.y)).r;
    let depth_b = textureSample(depth_texture, depth_sampler, uv - vec2<f32>(0.0, texel.y)).r;

    let pos_r = view_pos_from_depth(uv + vec2<f32>(texel.x, 0.0), depth_r);
    let pos_l = view_pos_from_depth(uv - vec2<f32>(texel.x, 0.0), depth_l);
    let pos_t = view_pos_from_depth(uv + vec2<f32>(0.0, texel.y), depth_t);
    let pos_b = view_pos_from_depth(uv - vec2<f32>(0.0, texel.y), depth_b);

    // Use the smaller difference to avoid artifacts at edges
    var ddx: vec3<f32>;
    if (abs(depth_r - depth) < abs(depth - depth_l)) {
        ddx = pos_r - frag_pos;
    } else {
        ddx = frag_pos - pos_l;
    }

    var ddy: vec3<f32>;
    if (abs(depth_t - depth) < abs(depth - depth_b)) {
        ddy = pos_t - frag_pos;
    } else {
        ddy = frag_pos - pos_b;
    }

    let normal = normalize(cross(ddy, ddx));

    // Random rotation vector from noise texture (tiled)
    let noise_uv = uv * params.noise_scale;
    let random_vec = textureSample(noise_texture, noise_sampler, noise_uv).xyz * 2.0 - 1.0;

    // Build TBN matrix via Gram-Schmidt
    let tangent = normalize(random_vec - normal * dot(random_vec, normal));
    let bitangent = cross(normal, tangent);
    let tbn = mat3x3<f32>(tangent, bitangent, normal);

    // Sample kernel and accumulate occlusion
    var occlusion = 0.0;
    let sample_count = 64;

    for (var i = 0; i < sample_count; i++) {
        // Transform sample from tangent space to view space
        let sample_dir = tbn * params.kernel[i].xyz;
        let sample_pos = frag_pos + sample_dir * params.radius;

        // Project sample to screen space
        let offset_clip = params.projection * vec4<f32>(sample_pos, 1.0);
        var offset_ndc = offset_clip.xy / offset_clip.w;
        let sample_uv = vec2<f32>(offset_ndc.x * 0.5 + 0.5, 1.0 - (offset_ndc.y * 0.5 + 0.5));

        // Sample depth at projected position
        let sample_depth = textureSample(depth_texture, depth_sampler, sample_uv).r;
        let sample_z = view_pos_from_depth(sample_uv, sample_depth).z;

        // Range check: only occlude if within radius
        let range_check = smoothstep(0.0, 1.0, params.radius / abs(frag_pos.z - sample_z));

        // Compare: is the sample behind the surface?
        if (sample_z >= sample_pos.z + params.bias) {
            occlusion += range_check;
        }
    }

    let ao = 1.0 - (occlusion / f32(sample_count)) * params.intensity;
    return clamp(ao, 0.0, 1.0);
}
