// Flint Skinned Mesh Shader
// Extends the PBR shader with GPU vertex skinning via bone matrices.
// Shares the same fragment shader (fs_main) as the standard pipeline.

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct MaterialUniforms {
    base_color: vec4<f32>,
    metallic: f32,
    roughness: f32,
    use_vertex_color: u32,
    debug_mode: u32,
    enable_tonemapping: u32,
    has_base_color_tex: u32,
    has_normal_map: u32,
    has_metallic_roughness_tex: u32,
    selection_highlight: u32,
    _pad_sel0: u32,
    _pad_sel1: u32,
    _pad_sel2: u32,
};

@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

@group(1) @binding(0)
var<uniform> material: MaterialUniforms;

@group(1) @binding(1)
var base_color_texture: texture_2d<f32>;
@group(1) @binding(2)
var base_color_sampler: sampler;

@group(1) @binding(3)
var normal_map_texture: texture_2d<f32>;
@group(1) @binding(4)
var normal_map_sampler: sampler;

@group(1) @binding(5)
var metallic_roughness_texture: texture_2d<f32>;
@group(1) @binding(6)
var metallic_roughness_sampler: sampler;

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

// Bind group 3: Bone matrices (storage buffer for arbitrary bone counts)
@group(3) @binding(0)
var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) joint_indices: vec4<u32>,
    @location(5) joint_weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

const PI: f32 = 3.14159265359;

fn skin_transform(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    return bone_matrices[joints.x] * weights.x
         + bone_matrices[joints.y] * weights.y
         + bone_matrices[joints.z] * weights.z
         + bone_matrices[joints.w] * weights.w;
}

@vertex
fn vs_skinned(in: SkinnedVertexInput) -> VertexOutput {
    var out: VertexOutput;

    let skin_mat = skin_transform(in.joint_indices, in.joint_weights);
    let skinned_pos = skin_mat * vec4<f32>(in.position, 1.0);
    let skinned_normal = normalize((skin_mat * vec4<f32>(in.normal, 0.0)).xyz);

    let world_pos = transform.model * skinned_pos;
    out.clip_position = transform.view_proj * world_pos;
    out.color = in.color;
    out.normal = normalize((transform.model_inv_transpose * vec4<f32>(skinned_normal, 0.0)).xyz);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}

// ===== Fragment shader (identical to shader.wgsl) =====

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

fn perturb_normal(N: vec3<f32>, world_pos: vec3<f32>, uv: vec2<f32>, map_normal: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(world_pos);
    let dp2 = dpdy(world_pos);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);

    let det = duv1.x * duv2.y - duv1.y * duv2.x;
    if (abs(det) < 0.0001) {
        return N;
    }
    let inv_det = 1.0 / det;
    let T = normalize((dp1 * duv2.y - dp2 * duv1.y) * inv_det);
    let B = normalize((dp2 * duv1.x - dp1 * duv2.x) * inv_det);

    let T_ortho = normalize(T - N * dot(N, T));
    let B_ortho = cross(N, T_ortho);

    return normalize(T_ortho * map_normal.x + B_ortho * map_normal.y + N * map_normal.z);
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

    if (shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0) {
        return 1.0;
    }

    let depth = proj.z;

    let texel_size = 1.0 / 1024.0;
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

    return shadow_sum / 9.0;
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
    let denominator = 4.0 * n_dot_v * n_dot_l + 0.0001;
    let specular = numerator / denominator;

    let kS = F;
    let kD = (vec3<f32>(1.0) - kS) * (1.0 - metallic);

    return (kD * albedo / PI + specular) * radiance * n_dot_l;
}

@fragment
fn fs_skinned(in: VertexOutput) -> @location(0) vec4<f32> {
    let dm = material.debug_mode;

    if (dm == 1u) {
        let N = normalize(in.normal);
        return vec4<f32>(N * 0.5 + 0.5, 1.0);
    }

    if (dm == 2u) {
        let d = in.clip_position.z;
        let linear = pow(d, 0.4);
        return vec4<f32>(vec3<f32>(linear), 1.0);
    }

    if (dm == 3u) {
        let freq = 8.0;
        let cx = floor(in.uv.x * freq);
        let cy = floor(in.uv.y * freq);
        let checker = ((cx + cy) % 2.0 + 2.0) % 2.0;
        let col = mix(vec3<f32>(0.15, 0.15, 0.15), vec3<f32>(0.95, 0.55, 0.95), checker);
        return vec4<f32>(col, 1.0);
    }

    if (dm == 4u) {
        var col: vec3<f32>;
        if (material.use_vertex_color == 1u) {
            col = in.color.rgb;
        } else {
            col = material.base_color.rgb;
        }
        return vec4<f32>(col, 1.0);
    }

    if (dm == 5u) {
        return vec4<f32>(material.metallic, material.roughness, 0.0, 1.0);
    }

    var albedo: vec3<f32>;
    var alpha: f32;
    if (material.use_vertex_color == 1u) {
        albedo = in.color.rgb;
        alpha = in.color.a;
    } else {
        albedo = material.base_color.rgb;
        alpha = material.base_color.a;
    }

    if (material.has_base_color_tex == 1u) {
        let tex_color = textureSample(base_color_texture, base_color_sampler, in.uv);
        albedo = tex_color.rgb * albedo;
        alpha = tex_color.a * alpha;
    }

    var metallic = material.metallic;
    var roughness = material.roughness;
    if (material.has_metallic_roughness_tex == 1u) {
        let mr = textureSample(metallic_roughness_texture, metallic_roughness_sampler, in.uv);
        roughness = mr.g * material.roughness;
        metallic = mr.b * material.metallic;
    }
    roughness = max(roughness, 0.04);

    var N = normalize(in.normal);
    if (material.has_normal_map == 1u) {
        let map_sample = textureSample(normal_map_texture, normal_map_sampler, in.uv);
        let map_normal = map_sample.rgb * 2.0 - 1.0;
        N = perturb_normal(N, in.world_pos, in.uv, map_normal);
    }

    let V = normalize(transform.camera_pos - in.world_pos);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let n_dot_v = max(dot(N, V), 0.001);
    let view_depth = length(in.world_pos - transform.camera_pos);

    var Lo = vec3<f32>(0.0);
    for (var i = 0u; i < lights.directional_count; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = normalize(light.direction);
        let radiance = light.color * light.intensity;
        var contribution = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);

        if (i == 0u) {
            let sf = shadow_factor(in.world_pos, view_depth);
            contribution = contribution * sf;
        }

        Lo += contribution;
    }

    for (var i = 0u; i < lights.point_count; i = i + 1u) {
        let light = lights.point_lights[i];
        let light_vec = light.position - in.world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        let atten = attenuation(distance, light.radius);
        let radiance = light.color * light.intensity * atten;
        Lo += evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);
    }

    for (var i = 0u; i < lights.spot_count; i = i + 1u) {
        let light = lights.spot_lights[i];
        let light_vec = light.position - in.world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        let atten = attenuation(distance, light.radius);
        let cone = spot_cone_factor(light_vec, light.direction, light.inner_angle, light.outer_angle);
        let radiance = light.color * light.intensity * atten * cone;
        Lo += evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v);
    }

    let sky_color = lights.ambient_sky.rgb;
    let ground_color = lights.ambient_ground.rgb;
    let hemisphere_mix = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(ground_color, sky_color, hemisphere_mix) * albedo;

    var color = ambient + Lo;

    // Selection highlight — Fresnel rim glow
    if (material.selection_highlight == 1u) {
        let rim = pow(1.0 - n_dot_v, 2.5);
        let rim_color = vec3<f32>(0.35, 0.65, 1.0);
        color = color + rim_color * rim * 0.7;
    }

    if (material.enable_tonemapping == 1u) {
        let mapped = aces_filmic(color);
        let corrected = linear_to_srgb(mapped);
        return vec4<f32>(corrected, alpha);
    }

    return vec4<f32>(color, alpha);
}
