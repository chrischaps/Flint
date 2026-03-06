// Volumetric lighting (god rays) ray-march shader
//
// Renders at half resolution. For each pixel, reconstructs world position from
// depth, then ray-marches from camera toward the pixel, sampling the shadow map
// at each step to accumulate in-scattered light from volumetric-enabled
// directional lights.

struct VolumetricUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    num_samples: f32,
    density: f32,
    max_distance: f32,
    decay: f32,
    near: f32,
    far: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0)
var<uniform> params: VolumetricUniforms;

// Group 1: Depth texture + nearest sampler
@group(1) @binding(0)
var depth_texture: texture_2d<f32>;
@group(1) @binding(1)
var depth_sampler_nn: sampler;

// Group 2: Shadow maps + comparison sampler + ShadowUniforms + LightUniforms
// Mirror the light structures from the PBR shader
struct DirectionalLight {
    direction: vec3<f32>,
    volumetric_intensity: f32,
    color: vec3<f32>,
    intensity: f32,
    volumetric_color: vec3<f32>,
    _pad1: f32,
};

struct PointLight {
    position: vec3<f32>,
    radius: f32,
    color: vec3<f32>,
    intensity: f32,
};

struct SpotLight {
    position: vec3<f32>,
    radius: f32,
    direction: vec3<f32>,
    inner_angle: f32,
    color: vec3<f32>,
    outer_angle: f32,
    intensity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

struct LightUniforms {
    directional_lights: array<DirectionalLight, 4>,
    point_lights: array<PointLight, 16>,
    spot_lights: array<SpotLight, 8>,
    directional_count: u32,
    point_count: u32,
    spot_count: u32,
    _pad: u32,
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
};

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>,
};

@group(2) @binding(0)
var shadow_maps: texture_depth_2d_array;
@group(2) @binding(1)
var shadow_sampler: sampler_comparison;
@group(2) @binding(2)
var<uniform> shadow: ShadowUniforms;
@group(2) @binding(3)
var<uniform> lights: LightUniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_volumetric(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    var out: VsOut;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Reconstruct world position from depth buffer
fn world_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let clip = vec4<f32>(uv * 2.0 - 1.0, depth, 1.0);
    let clip_y_flip = vec4<f32>(clip.x, -clip.y, clip.z, 1.0);
    let world = params.inv_view_proj * clip_y_flip;
    return world.xyz / world.w;
}

// 4x4 Bayer dither for ray jitter to reduce banding
fn bayer4(pos: vec2<f32>) -> f32 {
    let x = u32(pos.x) % 4u;
    let y = u32(pos.y) % 4u;
    let idx = (y * 4u + x);
    // 4x4 Bayer matrix values / 16
    let bayer = array<f32, 16>(
         0.0/16.0,  8.0/16.0,  2.0/16.0, 10.0/16.0,
        12.0/16.0,  4.0/16.0, 14.0/16.0,  6.0/16.0,
         3.0/16.0, 11.0/16.0,  1.0/16.0,  9.0/16.0,
        15.0/16.0,  7.0/16.0, 13.0/16.0,  5.0/16.0,
    );
    return bayer[idx];
}

// Henyey-Greenstein phase function for anisotropic scattering.
// g > 0 = forward scattering (toward light), g = 0 = isotropic, g < 0 = back scattering.
fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    return (1.0 - g2) / (4.0 * 3.14159265 * denom * sqrt(denom));
}

// Sample shadow map at a world position (single tap, no PCF)
fn shadow_at(world_pos: vec3<f32>, view_depth: f32) -> f32 {
    let shadow_far = shadow.cascade_splits.z;
    if (view_depth > shadow_far) {
        return 1.0;
    }

    var cascade: i32 = 0;
    if (view_depth > shadow.cascade_splits.x) {
        cascade = 1;
    }
    if (view_depth > shadow.cascade_splits.y) {
        cascade = 2;
    }

    let light_space = shadow.cascade_view_proj[cascade] * vec4<f32>(world_pos, 1.0);
    let proj = light_space.xyz / light_space.w;
    let shadow_uv = vec2<f32>(proj.x * 0.5 + 0.5, 0.5 - proj.y * 0.5);

    if (shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0) {
        return 1.0;
    }

    let depth = proj.z - 0.002;
    return textureSampleCompareLevel(shadow_maps, shadow_sampler, shadow_uv, cascade, depth);
}


// Compute view-space depth from world position
fn view_depth_from_world(world_pos: vec3<f32>) -> f32 {
    return length(world_pos - params.camera_pos);
}

@fragment
fn fs_volumetric(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // Sample depth
    let depth = textureSample(depth_texture, depth_sampler_nn, uv).r;

    let is_sky = depth >= 0.9999;
    var march_dist: f32;
    var ray_norm: vec3<f32>;

    if (is_sky) {
        // For sky pixels, march max_distance along the view ray
        let clip = vec4<f32>(uv * 2.0 - 1.0, 0.5, 1.0);
        let clip_y_flip = vec4<f32>(clip.x, -clip.y, clip.z, 1.0);
        let world = params.inv_view_proj * clip_y_flip;
        let far_pos = world.xyz / world.w;
        ray_norm = normalize(far_pos - params.camera_pos);
        march_dist = params.max_distance;
    } else {
        let world_pos = world_pos_from_depth(uv, depth);
        let ray_dir = world_pos - params.camera_pos;
        let ray_len = length(ray_dir);
        ray_norm = ray_dir / max(ray_len, 0.0001);
        march_dist = min(ray_len, params.max_distance);
    }

    let num_steps = u32(params.num_samples);
    let step_size = march_dist / f32(num_steps);

    // Jitter ray start with Bayer dither to reduce banding
    let jitter = bayer4(in.position.xy) * step_size;

    var accumulated = vec3<f32>(0.0, 0.0, 0.0);
    var transmittance = 1.0;

    let dir_count = min(lights.directional_count, 4u);

    for (var i = 0u; i < num_steps; i = i + 1u) {
        let t = (f32(i) + 0.5) * step_size + jitter;
        let sample_pos = params.camera_pos + ray_norm * t;
        let sample_view_depth = t;

        // Early exit when transmittance is negligible
        if (transmittance < 0.01) {
            break;
        }

        let shadow_vis = shadow_at(sample_pos, sample_view_depth);

        // Accumulate contribution from each volumetric directional light
        for (var li = 0u; li < dir_count; li = li + 1u) {
            let light = lights.directional_lights[li];
            if (light.volumetric_intensity > 0.0) {
                // Henyey-Greenstein forward scattering: bright shafts when
                // looking toward the light, dim when looking away.
                // light.direction points toward the light; ray_norm points
                // from camera into the scene.  cos_theta > 0 = facing light.
                let cos_theta = dot(ray_norm, normalize(light.direction));
                let phase = henyey_greenstein(cos_theta, 0.8);

                // Scale density by 0.01 so user-facing values ~1.0 give subtle beams
                let contribution = shadow_vis
                    * light.volumetric_intensity
                    * phase
                    * params.density * 0.01
                    * step_size;
                accumulated = accumulated + light.volumetric_color * contribution * transmittance;
            }
        }

        transmittance = transmittance * params.decay;
    }

    return vec4<f32>(accumulated, 1.0);
}
