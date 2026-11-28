// Flint terrain shader — splat-map texture blending + PBR lighting
// Uses the same Vertex format as the main PBR shader (position, normal, color, uv)
// Lighting is the scene's shared WGSL (lights.wgsl, light_bindings.wgsl,
// brdf.wgsl, shadows.wgsl, direct_light.wgsl, tonemap.wgsl), put in front of
// this file by `wgsl_sources::TERRAIN` (#738), and the transform group and a
// mesh's vertex products (mesh_vertex.wgsl, #1244), which the style mask
// redraws a chunk with.

struct TerrainUniforms {
    texture_tile: f32,
    metallic: f32,
    roughness: f32,
    enable_tonemapping: u32,
    // The renderer's debug view (`DebugMode::as_u32`, #1692): read by
    // `fs_debug` alone, so the lit path never branches on it.
    debug_mode: u32,
    _pad: u32,
    // Chunks along x and z, so `fs_wire` can find a chunk's border in the
    // terrain-wide UV.
    chunks: vec2<f32>,
};

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
    let world_pos = world_of(vec4<f32>(in.position, 1.0));
    out.clip_position = clip_of(world_pos);
    out.normal = world_normal_of(in.normal);
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

// --- Debug views (#1692) ---------------------------------------------------
//
// The renderer's debug views draw a chunk through these two fragment stages
// (`TerrainDebugPipelines`), never through `fs_main`, so `fs_main` stays the
// code it was and a frame with no debug view draws the same bytes. The splat
// blend below is `fs_main`'s, written out again for the same reason.

// The splat map's blend of the four layers at this fragment: `fs_main`'s albedo.
fn splat_albedo(in: VertexOutput) -> vec3<f32> {
    let splat = textureSample(splat_texture, splat_sampler, in.uv);
    let tile_uv = in.world_pos.xz * terrain.texture_tile / 100.0;
    let c0 = textureSample(layer0_texture, layer0_sampler, tile_uv).rgb;
    let c1 = textureSample(layer1_texture, layer1_sampler, tile_uv).rgb;
    let c2 = textureSample(layer2_texture, layer2_sampler, tile_uv).rgb;
    let c3 = textureSample(layer3_texture, layer3_sampler, tile_uv).rgb;
    let total_weight = max(splat.r + splat.g + splat.b + splat.a, 0.001);
    return (c0 * splat.r + c1 * splat.g + c2 * splat.b + c3 * splat.a) / total_weight;
}

// The mesh shader's debug modes (`shade` in pbr_surface.wgsl), for a chunk:
// 1 normals, 2 depth, 3 UV checker, 4 unlit (the splat's albedo), 5
// metallic (red) and roughness (green). Untonemapped, as the meshes' are.
@fragment
fn fs_debug(in: VertexOutput) -> @location(0) vec4<f32> {
    let dm = terrain.debug_mode;
    if (dm == 1u) {
        let N = normalize(in.normal);
        return vec4<f32>(N * 0.5 + 0.5, 1.0);
    }
    if (dm == 2u) {
        let linear = pow(in.clip_position.z, 0.4);
        return vec4<f32>(vec3<f32>(linear), 1.0);
    }
    if (dm == 3u) {
        // The terrain-wide UV (the splat map's), as a mesh's checker reads its own
        let freq = 8.0;
        let cx = floor(in.uv.x * freq);
        let cy = floor(in.uv.y * freq);
        let checker = ((cx + cy) % 2.0 + 2.0) % 2.0;
        let col = mix(vec3<f32>(0.15, 0.15, 0.15), vec3<f32>(0.95, 0.55, 0.95), checker);
        return vec4<f32>(col, 1.0);
    }
    if (dm == 5u) {
        return vec4<f32>(terrain.metallic, max(terrain.roughness, 0.04), 0.0, 1.0);
    }
    return vec4<f32>(splat_albedo(in), 1.0);
}

// A chunk's triangles as lines (the wireframe views): ink for every edge,
// and amber where the edge lies on a chunk's border, so the grid of chunks
// (and which of them the camera culled) reads at a glance. The terrain-wide
// UV is a whole number of chunks along a border, and only meets one at a
// vertex anywhere else.
@fragment
fn fs_wire(in: VertexOutput) -> @location(0) vec4<f32> {
    let g = in.uv * terrain.chunks;
    let off = abs(g - round(g));
    if (min(off.x, off.y) < 1.0e-4) {
        // Opaque: both chunks on a border draw it, and must agree
        return vec4<f32>(1.0, 0.45, 0.08, 1.0);
    }
    return vec4<f32>(0.015, 0.02, 0.03, 0.6);
}
