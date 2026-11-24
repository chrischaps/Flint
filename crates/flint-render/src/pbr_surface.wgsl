// The PBR surface both mesh shaders draw (#738): the material group, the
// material sampler, the fragment's inputs, and every fragment stage but the
// entry that takes `front_facing` (`fs_main` in shader.wgsl, `fs_skinned` /
// `fs_skinned_lighten` in skinned_shader.wgsl, each over its own vertex
// stage). Before #738 skinned_shader.wgsl held a copy of all of this, which
// had lost the clay levers and area sources. Needs `mesh_vertex.wgsl` (the
// transform group, #1244), `light_bindings.wgsl`, `direct_light.wgsl`,
// `tonemap.wgsl` and `fog_veil.wgsl`.

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
    // Emission (ADR 0076): rgb = the material's own light, w = per-vertex
    // emission scale (vertex alpha is then how much is surface).
    emissive: vec4<f32>,
    // 1 when `emissive_texture` holds the material's emissive map; its rgb
    // multiplies `emissive.rgb` (glTF `emissiveTexture`).
    has_emissive_tex: u32,
    // 1 when vertex alpha is the surface share, not opacity (ADR 0090), so
    // a strength of 0 leaves the light parts dark.
    vertex_emission: u32,
    // How much of a per-vertex light part is also lit surface (ADR 0096),
    // with its light unchanged.
    glow_surface: f32,
    // 1 for a glTF `doubleSided` material (#623): back faces flip N.
    double_sided: u32,
    // Multiplies the vertex colour, as surface and as per-vertex light
    // (ADR 0102: the entity's `material.vertex_tint_*`). w is unused.
    vertex_tint: vec4<f32>,
    // Unlit (#627, `material.unlit`): x = 1 skips lighting, drawing the
    // base colour times y (its `emissive_scale`) plus emission; 0 is lit.
    unlit: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> material: MaterialUniforms;

@group(1) @binding(1)
var base_color_texture: texture_2d<f32>;

@group(1) @binding(3)
var normal_map_texture: texture_2d<f32>;

@group(1) @binding(5)
var metallic_roughness_texture: texture_2d<f32>;

@group(1) @binding(7)
var emissive_texture: texture_2d<f32>;

// Every material's textures share this one sampler (linear, repeating). It
// lives here, bound once a frame, not in each material's group, so a material
// bind group spends no DX12 sampler descriptors (Capitol #577).
@group(2) @binding(4)
var material_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

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

// glTF `doubleSided` (#623): a back face is lit as the side the viewer sees,
// its normal turned round. Single-sided draws cull their back faces, so for
// them this is the normal as it came.
fn facing(input: VertexOutput, front: bool) -> VertexOutput {
    var v = input;
    if (material.double_sided == 1u && !front) {
        v.normal = -v.normal;
    }
    return v;
}

// The lit surface, both PBR shaders' colour: `fs_main` (shader.wgsl) and
// `fs_skinned` (skinned_shader.wgsl) hand it the fragment `facing` turned.
fn shade(in: VertexOutput) -> vec4<f32> {
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
            col = in.color.rgb * material.vertex_tint.rgb;
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
        albedo = in.color.rgb * material.vertex_tint.rgb;
        alpha = in.color.a;
    } else {
        albedo = material.base_color.rgb;
        alpha = material.base_color.a;
    }

    // Emission (ADR 0076). Per-vertex: alpha 1 is lit surface, 0 is pure
    // light giving off its rgb; vertex alpha is no longer opacity.
    // `glow_surface` (ADR 0096) lights the light part as surface too, and
    // `vertex_tint` (ADR 0102) colours its light as it does the surface.
    var emission = material.emissive.rgb;
    if (material.has_emissive_tex == 1u) {
        emission *= textureSample(emissive_texture, material_sampler, scaled_uv).rgb;
    }
    if (material.use_vertex_color == 1u && material.vertex_emission == 1u) {
        let glow = clamp(1.0 - in.color.a, 0.0, 1.0);
        emission += in.color.rgb * material.vertex_tint.rgb * glow * material.emissive.w;
        albedo = albedo * (1.0 - glow * (1.0 - material.glow_surface));
        alpha = 1.0;
    }

    // Sample base color texture if available
    if (material.has_base_color_tex == 1u) {
        let tex_color = textureSample(base_color_texture, material_sampler, scaled_uv);
        albedo = tex_color.rgb * albedo;
        alpha = tex_color.a * alpha;
    }

    // Metallic/roughness from texture or uniform
    var metallic = material.metallic;
    var roughness = material.roughness;
    if (material.has_metallic_roughness_tex == 1u) {
        let mr = textureSample(metallic_roughness_texture, material_sampler, scaled_uv);
        // glTF packing: green = roughness, blue = metallic
        roughness = mr.g * material.roughness;
        metallic = mr.b * material.metallic;
    }
    roughness = max(roughness, 0.04); // Clamp to avoid division by zero

    // Normal from interpolated vertex normal, optionally perturbed by normal map
    var N = normalize(in.normal);
    if (material.has_normal_map == 1u) {
        let map_sample = textureSample(normal_map_texture, material_sampler, scaled_uv);
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

    let Lo = direct_light(in.world_pos, normalize(in.normal), N, V, albedo, f0,
        metallic, roughness, n_dot_v, view_depth, in.clip_position.xy);

    // Hemisphere ambient from light uniforms
    let sky_color = lights.ambient_sky.rgb;
    let ground_color = lights.ambient_ground.rgb;
    let hemisphere_mix = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(ground_color, sky_color, hemisphere_mix) * albedo;

    // Emission goes on after lighting and before tonemapping, so it reads the
    // same under any light and bloom picks it up.
    var color = ambient + Lo + emission;
    // Unlit (#627): the base colour as it is, times `emissive_scale`, with
    // its emission; no lights, ambient or shadows reach it.
    if (material.unlit.x > 0.5) {
        color = albedo * material.unlit.y + emission;
    }

    let final_alpha = alpha * material.opacity * edge_feather(in);

    // Output linear HDR — tonemapping and gamma are applied in the composite pass.
    // When post-processing is disabled, the legacy tonemapping path runs here;
    // output stays LINEAR — the sRGB render target applies gamma encoding.
    if (material.enable_tonemapping == 1u) {
        let mapped = aces_filmic(color);
        return vec4<f32>(mapped, final_alpha);
    }

    return vec4<f32>(color, final_alpha);
}

// The style veil (#290): how much of the pixel this transparent surface
// covers, the same alpha fs_main draws with, for the entity style mask, so
// glass in front of a styled entity takes that share of the pixel back
// from the style. Only the material group and the light group's sampler
// are read; the targets are single-sample R8Unorm with "over" blending.
@fragment
fn fs_veil(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(veil_alpha(in));
}

fn veil_alpha(in: VertexOutput) -> f32 {
    let scaled_uv = in.uv * material.texture_scale;
    var alpha = material.base_color.a;
    if (material.use_vertex_color == 1u) {
        alpha = in.color.a;
        if (material.vertex_emission == 1u) {
            alpha = 1.0;
        }
    }
    if (material.has_base_color_tex == 1u) {
        alpha = textureSample(base_color_texture, material_sampler, scaled_uv).a * alpha;
    }
    return clamp(alpha * material.opacity * edge_feather(in), 0.0, 1.0);
}

// The fog veil (#513): this surface's weight (its coverage) and depth, as
// `fog_veil.wgsl` lays them out.
@fragment
fn fs_fog_veil(in: VertexOutput) -> @location(0) vec4<f32> {
    return fog_veil_texel(veil_alpha(in), in.clip_position.z);
}

// The fog veil of a surface that only adds light (additive)
@fragment
fn fs_fog_veil_light(in: VertexOutput) -> @location(0) vec4<f32> {
    return fog_veil_light_texel(veil_alpha(in), in.clip_position.z);
}

// The style albedo (#869): the surface's own colour, as fs_main builds it
// before any light reaches it, for a style that wants the light alone (the
// pencil's `tone = "light"` divides the picture by it). The glowing share of
// a vertex-emissive surface is no surface, so it is taken out as fs_main
// takes it out. Alpha 1 says a surface was drawn here; the target clears to 0.
@fragment
fn fs_albedo(in: VertexOutput) -> @location(0) vec4<f32> {
    let scaled_uv = in.uv * material.texture_scale;
    var albedo = material.base_color.rgb;
    if (material.use_vertex_color == 1u) {
        albedo = in.color.rgb * material.vertex_tint.rgb;
        if (material.vertex_emission == 1u) {
            let glow = clamp(1.0 - in.color.a, 0.0, 1.0);
            albedo = albedo * (1.0 - glow * (1.0 - material.glow_surface));
        }
    }
    if (material.has_base_color_tex == 1u) {
        albedo = textureSample(base_color_texture, material_sampler, scaled_uv).rgb * albedo;
    }
    return vec4<f32>(clamp(albedo, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// A soft silhouette (#582: a living prop's `softness`, in `unlit.z`): how
// much of the surface is left where it turns away from the eye. Facing the
// eye it's whole; at the outline, where the surface runs along the line of
// sight, it's gone, eased between over the first `softness` of |N.V|. So the
// edge feathers into what's behind it, still, at rest. The surface's own
// normal, not a normal map's, so the outline is the shape's. 0 is a hard edge
// and returns exactly 1.
fn edge_feather(in: VertexOutput) -> f32 {
    let softness = material.unlit.z;
    if (softness <= 0.0) {
        return 1.0;
    }
    let V = normalize(transform.camera_pos - in.world_pos);
    let facing_eye = abs(dot(normalize(in.normal), V));
    return smoothstep(0.0, softness, facing_eye);
}
