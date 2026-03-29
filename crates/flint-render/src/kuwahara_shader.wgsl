// Anisotropic Kuwahara Filter
//
// Uses the blurred structure tensor to orient 8 elliptical sectors along
// local edge directions. Each sector accumulates weighted mean and variance,
// then sectors are blended with low-variance sectors dominating.
// Based on Kyprianidis et al. "Anisotropic Kuwahara Filtering on the GPU"

struct KuwaharaUniforms {
    texel_size: vec2<f32>,
    radius: f32,
    sharpness: f32,
    hardness: f32,
    anisotropy: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

@group(2) @binding(0)
var tensor_texture: texture_2d<f32>;
@group(2) @binding(1)
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

const PI: f32 = 3.14159265359;
const NUM_SECTORS: u32 = 8u;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;
    let radius = i32(params.radius);

    // Sample blurred structure tensor (explicit LOD — no derivative requirement)
    let tensor = textureSampleLevel(tensor_texture, tensor_sampler, in.uv, 0.0);
    let E = tensor.r; // Jx*Jx
    let F = tensor.g; // Jx*Jy
    let G = tensor.b; // Jy*Jy

    // Eigenvalue decomposition of 2x2 symmetric matrix [[E,F],[F,G]]
    let disc = sqrt(max((E - G) * (E - G) + 4.0 * F * F, 0.0));
    let lambda1 = 0.5 * (E + G + disc);
    let lambda2 = 0.5 * (E + G - disc);

    // Orientation angle (direction of major eigenvector)
    let phi = 0.5 * atan2(2.0 * F, E - G);

    // Anisotropy measure [0,1]
    let A = (lambda1 - lambda2) / max(lambda1 + lambda2, 0.0001);
    let aniso = mix(1.0, 1.0 / max(1.0 - A * params.anisotropy, 0.1), params.anisotropy);

    // Rotation matrix for aligning sectors with edge direction
    let cos_phi = cos(phi);
    let sin_phi = sin(phi);

    // Per-sector accumulators
    var total_weight = 0.0;
    var total_color = vec3<f32>(0.0);

    let sector_angle = 2.0 * PI / f32(NUM_SECTORS);

    for (var s: u32 = 0u; s < NUM_SECTORS; s = s + 1u) {
        var mean = vec3<f32>(0.0);
        var mean_sq = vec3<f32>(0.0);
        var w_sum = 0.0;

        for (var y: i32 = -radius; y <= radius; y = y + 1) {
            for (var x: i32 = -radius; x <= radius; x = x + 1) {
                // Rotate sample position by -phi to align with edge
                let fx = f32(x);
                let fy = f32(y);
                let rx = fx * cos_phi + fy * sin_phi;
                let ry = -fx * sin_phi + fy * cos_phi;

                // Apply anisotropic scaling (stretch along edge direction)
                let sx = rx;
                let sy = ry * aniso;

                // Check if sample is within the elliptical radius and correct sector
                let dist = sqrt(sx * sx + sy * sy);
                if (dist <= f32(radius)) {
                    let sample_angle = atan2(sy, sx) + PI;
                    let sector_idx = u32(floor(sample_angle / sector_angle)) % NUM_SECTORS;

                    if (sector_idx == s) {
                        // Polynomial weight (Gaussian-like falloff from center)
                        let norm_dist = dist / f32(radius);
                        let w = pow(1.0 - norm_dist, params.hardness);

                        let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
                        let color = textureSampleLevel(hdr_texture, hdr_sampler, in.uv + offset, 0.0).rgb;

                        mean += color * w;
                        mean_sq += color * color * w;
                        w_sum += w;
                    }
                }
            }
        }

        // Compute sector mean and variance
        if (w_sum > 0.0) {
            mean /= w_sum;
            mean_sq /= w_sum;
            let variance = dot(mean_sq - mean * mean, vec3<f32>(1.0 / 3.0));
            let sector_weight = exp(-params.sharpness * max(variance, 0.0));
            total_color += mean * sector_weight;
            total_weight += sector_weight;
        }
    }

    if (total_weight > 0.0) {
        total_color /= total_weight;
    } else {
        total_color = textureSampleLevel(hdr_texture, hdr_sampler, in.uv, 0.0).rgb;
    }

    return vec4<f32>(total_color, 1.0);
}
