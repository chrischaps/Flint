// Flint terrain shader — splat-map texture blending + PBR lighting
// Uses the same Vertex format as the main PBR shader (position, normal, color, uv)
// Lighting is the scene's shared WGSL (lights.wgsl, light_bindings.wgsl,
// brdf.wgsl, shadows.wgsl, direct_light.wgsl, tonemap.wgsl), put in front of
// this file by `wgsl_sources::TERRAIN` (#738).

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

// Bind group 2: Lights + shadows (light_bindings.wgsl)

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

    // View depth for shadow cascade selection
    let view_depth = length(transform.camera_pos - in.world_pos);

    // The scene's lights, through the meshes' own loops (#738): terrain now
    // takes the cascades' normal offset, area sources and the clay levers.
    // No normal map, so the shading normal is the surface's own.
    var color = direct_light(in.world_pos, N, N, V, albedo, f0, metallic, roughness,
        n_dot_v, view_depth, in.clip_position.xy);

    // Hemisphere ambient
    let sky_weight = dot(N, vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + 0.5;
    let ambient = mix(lights.ambient_ground.rgb, lights.ambient_sky.rgb, sky_weight) * albedo;
    color = color + ambient;

    // Tonemapping (only when post-processing is disabled)
    // Output stays LINEAR — the sRGB render target applies gamma encoding.
    if (terrain.enable_tonemapping == 1u) {
        color = aces_filmic(color);
    }

    return vec4<f32>(color, 1.0);
}
