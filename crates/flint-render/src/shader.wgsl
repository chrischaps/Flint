// Flint PBR scene viewer shader
// Cook-Torrance BRDF with metallic-roughness workflow

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
    opacity: f32,
    alpha_cutoff: f32,
    texture_scale: f32,
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
    // Physical source radius (world units); 0 = punctual (ADR 0056).
    // Struct grew 32 -> 48 B — must match the Rust PointLight in all six
    // LightUniforms mirrors (light_uniforms_layout test).
    source_radius: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

struct SpotLight {
    position: vec3<f32>,
    radius: f32,
    direction: vec3<f32>,
    inner_angle: f32,
    color: vec3<f32>,
    outer_angle: f32,
    intensity: f32,
    // Physical source radius (world units); 0 = punctual (ADR 0056).
    // Rides the former _pad0 slot — layout unchanged; the other five
    // mirrors keep the _pad0 name (they never read it).
    source_radius: f32,
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
    // rgb = sheen tint, w = strength; zero = off (must match LightUniforms in pipeline.rs)
    sheen_color_strength: vec4<f32>,
};

@group(2) @binding(0)
var<uniform> lights: LightUniforms;

@group(2) @binding(1)
var shadow_maps: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>, // 3 splits + padding
    // PCSS (ADR 0057): xyz = per-cascade light-ortho depth range (world
    // units), w = tan(sun angular size); w = 0 -> legacy 3x3 PCF verbatim.
    pcss: vec4<f32>,
};

@group(2) @binding(3)
var<uniform> shadow: ShadowUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

const PI: f32 = 3.14159265359;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let world_pos = transform.model * vec4<f32>(in.position, 1.0);
    out.clip_position = transform.view_proj * world_pos;
    out.color = in.color;
    out.normal = normalize((transform.model_inv_transpose * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_pos = world_pos.xyz;
    out.uv = in.uv;

    return out;
}

// GGX/Trowbridge-Reitz Normal Distribution Function
fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let denom_term = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom_term * denom_term);
}

// Smith-Schlick Geometry function (single direction)
fn geometry_schlick(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

// Smith's method combining both view and light directions
fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick(n_dot_v, roughness) * geometry_schlick(n_dot_l, roughness);
}

// Schlick Fresnel approximation
fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

// ACES filmic tone mapping curve
fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate((x * (a * x + b)) / (x * (c * x + d) + e));
}

// Linear to sRGB gamma correction
fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2));
}

// Perturb normal using a tangent-space normal map via screen-space derivatives
fn perturb_normal(N: vec3<f32>, world_pos: vec3<f32>, uv: vec2<f32>, map_normal: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(world_pos);
    let dp2 = dpdy(world_pos);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);

    // Guard only against a truly degenerate UV parameterisation. The
    // derivatives are per screen pixel, so a 1k texture magnified over a
    // few hundred pixels yields det ~ 1e-5 to 1e-6; an absolute 1e-4
    // threshold silently discarded every normal map on a close-up mesh.
    let det = duv1.x * duv2.y - duv1.y * duv2.x;
    if (abs(det) < 1e-12) {
        return N;
    }
    let inv_det = 1.0 / det;
    let T = normalize((dp1 * duv2.y - dp2 * duv1.y) * inv_det);
    let B = normalize((dp2 * duv1.x - dp1 * duv2.x) * inv_det);

    // Re-orthogonalize T with respect to N
    let T_ortho = normalize(T - N * dot(N, T));
    let B_ortho = cross(N, T_ortho);

    return normalize(T_ortho * map_normal.x + B_ortho * map_normal.y + N * map_normal.z);
}

// Smooth distance attenuation with radius falloff
fn attenuation(distance: f32, radius: f32) -> f32 {
    let d2 = distance * distance;
    let r2 = radius * radius;
    let factor = d2 / r2;
    let falloff = saturate(1.0 - factor * factor);
    return falloff * falloff / max(d2, 0.0001);
}

// Spotlight cone falloff
fn spot_cone_factor(light_to_frag: vec3<f32>, spot_dir: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    let cos_angle = dot(normalize(light_to_frag), normalize(spot_dir));
    return saturate((cos_angle - cos_outer) / max(cos_inner - cos_outer, 0.0001));
}

// Compute shadow factor for a world-space position using cascaded shadow maps
// Returns 1.0 (fully lit) or a value approaching 0.0 (shadowed)
// N is the geometric surface normal, used for normal-offset receiver bias:
// slope-scale bias in the depth pass alone can't prevent acne at glancing
// angles on curved surfaces (low-poly cylinders/capsules band visibly).
fn shadow_factor(world_pos: vec3<f32>, view_depth: f32, N: vec3<f32>) -> f32 {
    // cascade_splits.z is the shadow far plane — fade out over the last 15%
    let shadow_far = shadow.cascade_splits.z;
    let fade_start = shadow_far * 0.75;
    if (view_depth > shadow_far) {
        return 1.0;
    }
    let distance_fade = 1.0 - smoothstep(fade_start, shadow_far, view_depth);

    // Select cascade based on view-space depth
    var cascade: i32 = 0;
    if (view_depth > shadow.cascade_splits.x) {
        cascade = 1;
    }
    if (view_depth > shadow.cascade_splits.y) {
        cascade = 2;
    }

    // Normal-offset bias: push the receiver position out along the surface
    // normal by ~2 shadow texels (in world units) before projecting. The
    // cascade's world-per-texel comes from the ortho projection row scale.
    // Texel size rides cascade_splits.w (0 = unset -> legacy hardcoded
    // 1/2048, the exact pre-lever behavior at the default 2048 resolution;
    // powers of two keep the fallback bit-identical).
    var texel_size = shadow.cascade_splits.w;
    if (texel_size <= 0.0) {
        texel_size = 1.0 / 2048.0;
    }

    let m = shadow.cascade_view_proj[cascade];
    let row0_len = length(vec3<f32>(m[0].x, m[1].x, m[2].x));
    let texel_world = 2.0 * texel_size / max(row0_len, 0.0001);
    let biased_pos = world_pos + N * texel_world * 2.0;

    // Project world position into shadow map space
    let light_space = shadow.cascade_view_proj[cascade] * vec4<f32>(biased_pos, 1.0);
    let proj = light_space.xyz / light_space.w;

    // Convert from clip space [-1,1] to texture space [0,1]
    let shadow_uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);

    // Fade at shadow map UV edges to avoid hard boundary
    let edge_fade = min(
        min(smoothstep(0.0, 0.02, shadow_uv.x), smoothstep(0.0, 0.02, 1.0 - shadow_uv.x)),
        min(smoothstep(0.0, 0.02, shadow_uv.y), smoothstep(0.0, 0.02, 1.0 - shadow_uv.y))
    );
    if (edge_fade <= 0.0) {
        return 1.0;
    }

    let depth = proj.z;

    var raw_shadow = 1.0;
    if (shadow.pcss.w > 0.0) {
        // === PCSS: penumbra grows with occluder distance (ADR 0057) ===
        // pcss.w = tan(sun angular size); pcss[cascade] = the light-ortho
        // depth range in world units (converts [0,1] depth deltas back to
        // world distances). The ortho row scale converts world -> UV.
        let tan_a = shadow.pcss.w;
        let depth_range = shadow.pcss[cascade];
        let uv_per_world = row0_len * 0.5;
        let max_radius_uv = 8.0 * texel_size; // kernel-cost / cascade-bleed cap
        let resolution = 1.0 / texel_size;
        let max_coord = i32(resolution) - 1;

        // Blocker search: average occluder depth over a Vogel disk sized by
        // how far a blocker across the whole depth range could spread the
        // penumbra at this receiver. textureLoad (raw depth reads) — the
        // comparison sampler can't return depths and Depth32Float is not
        // filterable, so no new binding is needed.
        let search_uv = clamp(tan_a * depth * depth_range * uv_per_world,
            texel_size, max_radius_uv);
        var blocker_sum = 0.0;
        var blocker_count = 0.0;
        for (var i = 0; i < 16; i = i + 1) {
            // Vogel disk: r = sqrt((i+0.5)/N), golden-angle steps.
            let r = sqrt((f32(i) + 0.5) / 16.0);
            let theta = f32(i) * 2.39996;
            let tap_uv = shadow_uv + vec2<f32>(cos(theta), sin(theta)) * r * search_uv;
            let coords = clamp(vec2<i32>(tap_uv * resolution),
                vec2<i32>(0), vec2<i32>(max_coord));
            let d = textureLoad(shadow_maps, coords, cascade, 0);
            if (d < depth) {
                blocker_sum += d;
                blocker_count += 1.0;
            }
        }

        if (blocker_count > 0.0) {
            // Directional/ortho penumbra: width = tan(angular) x world-space
            // occluder distance. No divide (that's the point-light form) —
            // the clamps below are the firefly-lesson guards (ADR 0048).
            let avg_blocker = blocker_sum / blocker_count;
            let penumbra_world = tan_a * max(depth - avg_blocker, 0.0) * depth_range;
            let filter_uv = clamp(penumbra_world * uv_per_world,
                texel_size, max_radius_uv);

            var shadow_sum = 0.0;
            for (var i = 0; i < 16; i = i + 1) {
                let r = sqrt((f32(i) + 0.5) / 16.0);
                let theta = f32(i) * 2.39996;
                let offset = vec2<f32>(cos(theta), sin(theta)) * r * filter_uv;
                shadow_sum += textureSampleCompareLevel(
                    shadow_maps,
                    shadow_sampler,
                    shadow_uv + offset,
                    cascade,
                    depth
                );
            }
            raw_shadow = shadow_sum / 16.0;
        }
        // No blockers found -> fully lit (raw_shadow stays 1.0).
    } else {
        // Legacy 3x3 PCF (percentage-closer filtering) — verbatim pre-PCSS
        // path; pcss.w = 0 means the lever is off (ADR 0057).
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
        raw_shadow = shadow_sum / 9.0;
    }

    // Blend shadow toward 1.0 (lit) based on distance and edge fades
    return mix(1.0, raw_shadow, distance_fade * edge_fade);
}

// Evaluate a single directional light using Cook-Torrance BRDF.
// `wrap` softens only the diffuse terminator (wrap-diffuse, a cheap
// subsurface-ish cue for matte materials); specular keeps the true n·l.
// `oren` blends the diffuse magnitude from Lambert toward the Fujii
// qualitative Oren-Nayar approximation (sigma = material roughness) —
// a flatter, chalkier falloff for rough matte surfaces. The two levers
// are orthogonal: wrap replaces the diffuse n·l, Oren-Nayar scales the
// diffuse magnitude computed from the raw geometric angles.
// wrap = 0 and oren = 0 take the original code path exactly.
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
    wrap: f32,
    oren: f32,
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

    let sheen_strength = lights.sheen_color_strength.w;
    if (wrap <= 0.0 && oren <= 0.0 && sheen_strength <= 0.0) {
        return (kD * albedo / PI + specular) * radiance * n_dot_l;
    }
    // Charlie sheen rim (Estevez & Kulla 2017 NDF, fixed sheen roughness,
    // no visibility term — a velvety grazing response, tinted per scene;
    // masked by n·l so it reads as rim under each light, not a screen glow).
    var sheen = vec3<f32>(0.0);
    if (sheen_strength > 0.0) {
        let alpha_s = 0.5;
        let sin2h = max(1.0 - n_dot_h * n_dot_h, 1e-4);
        let d_charlie = (2.0 + 1.0 / alpha_s) * pow(sin2h, 0.5 / alpha_s) / (2.0 * PI);
        sheen = lights.sheen_color_strength.rgb * sheen_strength * d_charlie;
    }
    // Wrap replaces the diffuse n·l (terminator softening only).
    var n_dot_l_diffuse = n_dot_l;
    if (wrap > 0.0) {
        n_dot_l_diffuse = max((dot(N, L) + wrap) / (1.0 + wrap), 0.0);
    }
    // Fujii's energy-conserving qualitative Oren-Nayar; `oren` blends.
    var diffuse_scale = 1.0;
    if (oren > 0.0) {
        let sigma = roughness;
        let fujii_a = 1.0 / (1.0 + (0.5 - 2.0 / (3.0 * PI)) * sigma);
        let fujii_b = sigma * fujii_a;
        let s = dot(L, V) - n_dot_l * n_dot_v;
        let t = select(1.0, max(n_dot_l, n_dot_v), s > 0.0);
        // Clamp s/t to 1: with normal-mapped grazing angles both n·l and
        // n·v can approach 0 while s stays positive, exploding the ratio
        // into white fireflies (and wrap-diffuse removes the n·l damping
        // that would normally hide them). The physical range is [0, 1].
        let s_over_t = clamp(max(s, 0.0) / max(t, 1e-4), 0.0, 1.0);
        diffuse_scale = mix(1.0, fujii_a + fujii_b * s_over_t, oren);
    }
    return (kD * albedo / PI) * diffuse_scale * radiance * n_dot_l_diffuse
        + (specular + sheen) * radiance * n_dot_l;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // --- Debug visualization early returns ---
    let dm = material.debug_mode;

    // Mode 1: Normals — world-space normal mapped to RGB
    if (dm == 1u) {
        let N = normalize(in.normal);
        return vec4<f32>(N * 0.5 + 0.5, 1.0);
    }

    // Mode 2: Depth — linearized clip-space Z as grayscale with gamma
    if (dm == 2u) {
        let d = in.clip_position.z;
        let linear = pow(d, 0.4); // gamma curve for better contrast
        return vec4<f32>(vec3<f32>(linear), 1.0);
    }

    // Mode 3: UV Checker — procedural checkerboard from UV coordinates
    if (dm == 3u) {
        let freq = 8.0;
        let cx = floor(in.uv.x * freq);
        let cy = floor(in.uv.y * freq);
        let checker = ((cx + cy) % 2.0 + 2.0) % 2.0; // ensure positive modulo
        let col = mix(vec3<f32>(0.15, 0.15, 0.15), vec3<f32>(0.95, 0.55, 0.95), checker);
        return vec4<f32>(col, 1.0);
    }

    // Mode 4: Unlit — albedo only, no lighting
    if (dm == 4u) {
        var col: vec3<f32>;
        if (material.use_vertex_color == 1u) {
            col = in.color.rgb;
        } else {
            col = material.base_color.rgb;
        }
        return vec4<f32>(col, 1.0);
    }

    // Mode 5: Metallic/Roughness — red=metallic, green=roughness
    if (dm == 5u) {
        return vec4<f32>(material.metallic, material.roughness, 0.0, 1.0);
    }

    // --- Standard PBR path (mode 0) ---

    // Scale UVs for texture tiling
    let scaled_uv = in.uv * material.texture_scale;

    // Determine base color from vertex color, texture, or material uniform
    var albedo: vec3<f32>;
    var alpha: f32;
    if (material.use_vertex_color == 1u) {
        albedo = in.color.rgb;
        alpha = in.color.a;
    } else {
        albedo = material.base_color.rgb;
        alpha = material.base_color.a;
    }

    // Sample base color texture if available
    if (material.has_base_color_tex == 1u) {
        let tex_color = textureSample(base_color_texture, base_color_sampler, scaled_uv);
        albedo = tex_color.rgb * albedo;
        alpha = tex_color.a * alpha;
    }

    // Metallic/roughness from texture or uniform
    var metallic = material.metallic;
    var roughness = material.roughness;
    if (material.has_metallic_roughness_tex == 1u) {
        let mr = textureSample(metallic_roughness_texture, metallic_roughness_sampler, scaled_uv);
        // glTF packing: green = roughness, blue = metallic
        roughness = mr.g * material.roughness;
        metallic = mr.b * material.metallic;
    }
    roughness = max(roughness, 0.04); // Clamp to avoid division by zero

    // Normal from interpolated vertex normal, optionally perturbed by normal map
    var N = normalize(in.normal);
    if (material.has_normal_map == 1u) {
        let map_sample = textureSample(normal_map_texture, normal_map_sampler, scaled_uv);
        let map_normal = map_sample.rgb * 2.0 - 1.0;
        N = perturb_normal(N, in.world_pos, scaled_uv, map_normal);
    }

    let V = normalize(transform.camera_pos - in.world_pos);

    // F0: reflectance at normal incidence
    // Dielectrics reflect ~4%, metals reflect their albedo
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);

    let n_dot_v = max(dot(N, V), 0.001); // Avoid zero

    // View-space depth for cascade selection (distance from camera along view direction)
    let view_depth = length(in.world_pos - transform.camera_pos);

    // === Directional lights ===
    // Diffuse-wrap knob rides ambient_sky.w, encoded as (1 + wrap) so every
    // legacy CPU write of 1.0 decodes to wrap = 0 (exact original shading).
    // Oren-Nayar blend rides ambient_ground.w the same way (ADR 0048).
    let wrap = max(lights.ambient_sky.w - 1.0, 0.0);
    let oren = max(lights.ambient_ground.w - 1.0, 0.0);
    var Lo = vec3<f32>(0.0);
    for (var i = 0u; i < lights.directional_count; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = normalize(light.direction);
        let radiance = light.color * light.intensity;
        var contribution = evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness, n_dot_v, wrap, oren);

        // Apply shadow from cascaded shadow maps to the first directional light
        if (i == 0u) {
            let sf = shadow_factor(in.world_pos, view_depth, normalize(in.normal));
            contribution = contribution * sf;
        }

        Lo += contribution;
    }

    // === Point lights ===
    for (var i = 0u; i < lights.point_count; i = i + 1u) {
        let light = lights.point_lights[i];
        let light_vec = light.position - in.world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        // Representative-point area source (ADR 0056): a source of radius r
        // seen from distance d widens the specular lobe by ~r/(2d), and the
        // shading distance never falls inside the source. source_radius = 0
        // passes the original arguments verbatim.
        var roughness_l = roughness;
        var shade_dist = distance;
        if (light.source_radius > 0.0) {
            roughness_l = clamp(
                roughness + light.source_radius / (2.0 * max(distance, light.source_radius)),
                roughness, 1.0);
            shade_dist = max(distance, light.source_radius);
        }
        let atten = attenuation(shade_dist, light.radius);
        let radiance = light.color * light.intensity * atten;
        Lo += evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness_l, n_dot_v, wrap, oren);
    }

    // === Spot lights ===
    for (var i = 0u; i < lights.spot_count; i = i + 1u) {
        let light = lights.spot_lights[i];
        let light_vec = light.position - in.world_pos;
        let distance = length(light_vec);
        let L = normalize(light_vec);
        // Representative-point area source — same construction as the point
        // loop above (ADR 0056); 0 = verbatim legacy.
        var roughness_l = roughness;
        var shade_dist = distance;
        if (light.source_radius > 0.0) {
            roughness_l = clamp(
                roughness + light.source_radius / (2.0 * max(distance, light.source_radius)),
                roughness, 1.0);
            shade_dist = max(distance, light.source_radius);
        }
        let atten = attenuation(shade_dist, light.radius);
        let cone = spot_cone_factor(light_vec, light.direction, light.inner_angle, light.outer_angle);
        let radiance = light.color * light.intensity * atten * cone;
        Lo += evaluate_light(N, V, L, radiance, albedo, f0, metallic, roughness_l, n_dot_v, wrap, oren);
    }

    // Hemisphere ambient from light uniforms
    let sky_color = lights.ambient_sky.rgb;
    let ground_color = lights.ambient_ground.rgb;
    let hemisphere_mix = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(ground_color, sky_color, hemisphere_mix) * albedo;

    var color = ambient + Lo;

    let final_alpha = alpha * material.opacity;

    // Output linear HDR — tonemapping and gamma are applied in the composite pass.
    // When post-processing is disabled, the legacy tonemapping path runs here;
    // output stays LINEAR — the sRGB render target applies gamma encoding.
    if (material.enable_tonemapping == 1u) {
        let mapped = aces_filmic(color);
        return vec4<f32>(mapped, final_alpha);
    }

    return vec4<f32>(color, final_alpha);
}
