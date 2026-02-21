// Flint terrain shader — splat-map texture blending + PBR lighting
// Uses the same Vertex format as the main PBR shader (position, normal, color, uv)

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct TerrainUniforms {
    texture_tile: f32,
    metallic: f32,
    roughness: f32,
    enable_tonemapping: u32,
};

// Bind group 0: Transform (shared with PBR)
@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Bind group 1: Terrain material
@group(1) @binding(0)
var<uniform> terrain: TerrainUniforms;

@group(1) @binding(1)
var splat_texture: texture_2d<f32>;
@group(1) @binding(2)
var splat_sampler: sampler;

@group(1) @binding(3)
var layer0_texture: texture_2d<f32>;
@group(1) @binding(4)
var layer0_sampler: sampler;

@group(1) @binding(5)
var layer1_texture: texture_2d<f32>;
@group(1) @binding(6)
var layer1_sampler: sampler;

@group(1) @binding(7)
var layer2_texture: texture_2d<f32>;
@group(1) @binding(8)
var layer2_sampler: sampler;

@group(1) @binding(9)
var layer3_texture: texture_2d<f32>;
@group(1) @binding(10)
var layer3_sampler: sampler;

// Bind group 2: Lights + shadows (shared with PBR)
struct DirectionalLight {
    direction: vec3<f32>,
    _pad0: f32,
    color: vec3<f32>,
    intensity: f32,
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

@group(2) @binding(0)
var<uniform> lights: LightUniforms;

@group(2) @binding(1)
var shadow_maps: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>,
};

@group(2) @binding(3)
var<uniform> shadow: ShadowUniforms;

// Vertex I/O
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

const PI: f32 = 3.14159265359;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    out.clip_position = transform.view_proj * world_pos;
    out.normal = normalize((transform.model_inv_transpose * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;
    return out;
}

// PBR lighting functions (same as shader.wgsl)
fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let denom_term = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom_term * denom_term);
}

fn geometry_schlick(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick(n_dot_v, roughness) * geometry_schlick(n_dot_l, roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2));
}

fn attenuation(distance: f32, radius: f32) -> f32 {
    let d2 = distance * distance;
    let r2 = radius * radius;
    let factor = d2 / r2;
    let falloff = saturate(1.0 - factor * factor);
    return falloff * falloff / max(d2, 0.0001);
}

fn spot_cone_factor(light_to_frag: vec3<f32>, spot_dir: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    let cos_angle = dot(normalize(light_to_frag), normalize(spot_dir));
    return saturate((cos_angle - cos_outer) / max(cos_inner - cos_outer, 0.0001));
}

fn shadow_factor(world_pos: vec3<f32>, view_depth: f32) -> f32 {
    let shadow_far = shadow.cascade_splits.z;
    let fade_start = shadow_far * 0.75;
    if (view_depth > shadow_far) {
        return 1.0;
    }
    let distance_fade = 1.0 - smoothstep(fade_start, shadow_far, view_depth);

    var cascade: i32 = 0;
    if (view_depth > shadow.cascade_splits.x) {
        cascade = 1;
    }
    if (view_depth > shadow.cascade_splits.y) {
        cascade = 2;
    }

    let light_space = shadow.cascade_view_proj[cascade] * vec4<f32>(world_pos, 1.0);
    let proj = light_space.xyz / light_space.w;
    let shadow_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);

    let edge_fade = min(
        min(smoothstep(0.0, 0.02, shadow_uv.x), smoothstep(0.0, 0.02, 1.0 - shadow_uv.x)),
        min(smoothstep(0.0, 0.02, shadow_uv.y), smoothstep(0.0, 0.02, 1.0 - shadow_uv.y))
    );
    if (edge_fade <= 0.0) {
        return 1.0;
    }

    let depth = proj.z;
    let texel_size = 1.0 / 2048.0;
    var shadow_sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            shadow_sum += textureSampleCompareLevel(
                shadow_maps,
                shadow_sampler,
                shadow_uv + offset,
                cascade,
                depth
            );
        }
    }

    let raw_shadow = shadow_sum / 9.0;
    return mix(1.0, raw_shadow, distance_fade * edge_fade);
}

fn evaluate_light(
    N: vec3<f32>,
    V: vec3<f32>,
    L: vec3<f32>,
    radiance: vec3<f32>,
    albedo: vec3<f32>,
    f0: vec3<f32>,
    metallic: f32,
    roughness: f32,
    n_dot_v: f32,
) -> vec3<f32> {
    let H = normalize(V + L);
    let n_dot_l = max(dot(N, L), 0.0);
    let n_dot_h = max(dot(N, H), 0.0);
    let h_dot_v = max(dot(H, V), 0.0);

    let D = distribution_ggx(n_dot_h, roughness);
    let G = geometry_smith(n_dot_v, n_dot_l, roughness);
    let F = fresnel_schlick(h_dot_v, f0);

    let numerator = D * G * F;
    let denominator = max(4.0 * n_dot_v * n_dot_l, 0.001);
    let specular = numerator / denominator;

    let k_d = (vec3<f32>(1.0) - F) * (1.0 - metallic);
    let diffuse = k_d * albedo / PI;

    return (diffuse + specular) * radiance * n_dot_l;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let N = normalize(in.normal);
    let V = normalize(transform.camera_pos - in.world_pos);
    let n_dot_v = max(dot(N, V), 0.0);

    // Sample splat map at terrain UV (normalized over entire terrain)
    let splat = textureSample(splat_texture, splat_sampler, in.uv);

    // Compute tiling UV from world position
    let tile_uv = in.world_pos.xz * terrain.texture_tile / 100.0;

    // Sample layer textures
    let c0 = textureSample(layer0_texture, layer0_sampler, tile_uv).rgb;
    let c1 = textureSample(layer1_texture, layer1_sampler, tile_uv).rgb;
    let c2 = textureSample(layer2_texture, layer2_sampler, tile_uv).rgb;
    let c3 = textureSample(layer3_texture, layer3_sampler, tile_uv).rgb;

    // Blend by splat weights (normalize to avoid darkening)
    let total_weight = max(splat.r + splat.g + splat.b + splat.a, 0.001);
    let albedo = (c0 * splat.r + c1 * splat.g + c2 * splat.b + c3 * splat.a) / total_weight;

    let metallic = terrain.metallic;
    let roughness = max(terrain.roughness, 0.04);

    // Compute f0 (base reflectance)
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);

    var color = vec3<f32>(0.0);

    // View depth for shadow cascade selection
    let view_depth = length(transform.camera_pos - in.world_pos);

    // Directional lights
    for (var i = 0u; i < lights.directional_count; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = normalize(light.direction);
        let radiance = light.color * light.intensity;

        var contrib = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);

        // Apply shadow from first directional light
        if (i == 0u) {
            let sf = shadow_factor(in.world_pos, view_depth);
            contrib = contrib * sf;
        }

        color = color + contrib;
    }

    // Point lights
    for (var i = 0u; i < lights.point_count; i = i + 1u) {
        let light = lights.point_lights[i];
        let to_light = light.position - in.world_pos;
        let dist = length(to_light);
        let L = to_light / dist;
        let atten = attenuation(dist, light.radius);
        let radiance = light.color * light.intensity * atten;

        color = color + evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);
    }

    // Spot lights
    for (var i = 0u; i < lights.spot_count; i = i + 1u) {
        let light = lights.spot_lights[i];
        let to_light = light.position - in.world_pos;
        let dist = length(to_light);
        let L = to_light / dist;
        let atten = attenuation(dist, light.radius);
        let cone = spot_cone_factor(-L, light.direction, light.inner_angle, light.outer_angle);
        let radiance = light.color * light.intensity * atten * cone;

        color = color + evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);
    }

    // Hemisphere ambient
    let sky_weight = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(lights.ambient_ground.rgb, lights.ambient_sky.rgb, sky_weight) * albedo;
    color = color + ambient;

    // Tonemapping (only when post-processing is disabled)
    if (terrain.enable_tonemapping == 1u) {
        color = aces_filmic(color);
        color = linear_to_srgb(color);
    }

    return vec4<f32>(color, 1.0);
}
