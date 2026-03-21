// crates/flint-render/src/grass_compute.wgsl
// Grass placement compute shader — scatters instances based on splat map + heightmap

struct GrassComputeUniforms {
    camera_pos: vec3<f32>,
    time: f32,
    terrain_offset: vec3<f32>,
    density: f32,
    terrain_width: f32,
    terrain_depth: f32,
    height_scale: f32,
    max_distance: f32,
    fade_start: f32,
    density_threshold: f32,
    density_layer: u32,
    blade_height: f32,
    height_variation: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

struct GrassInstance {
    position: vec3<f32>,
    rotation: f32,
    height: f32,
    tint: u32,
};

// Group 0: Uniforms
@group(0) @binding(0)
var<uniform> params: GrassComputeUniforms;

// Group 1: Terrain textures
@group(1) @binding(0)
var heightmap_texture: texture_2d<f32>;
@group(1) @binding(1)
var heightmap_sampler: sampler;
@group(1) @binding(2)
var splat_texture: texture_2d<f32>;
@group(1) @binding(3)
var splat_sampler: sampler;

// Group 2: Instance output
@group(2) @binding(0)
var<storage, read_write> instances: array<GrassInstance>;
@group(2) @binding(1)
var<storage, read_write> instance_count: atomic<u32>;

// Hash function for deterministic pseudo-random values from position
fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let n = vec2<f32>(
        dot(p, vec2<f32>(127.1, 311.7)),
        dot(p, vec2<f32>(269.5, 183.3))
    );
    return fract(sin(n) * 43758.5453);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    // Grid spacing from density: spacing = 1 / sqrt(density)
    let spacing = 1.0 / sqrt(params.density);

    // Grid dimensions
    let grid_x = u32(ceil(params.terrain_width / spacing));
    let grid_z = u32(ceil(params.terrain_depth / spacing));

    if gid.x >= grid_x || gid.y >= grid_z {
        return;
    }

    // Base position on grid
    let base_x = f32(gid.x) * spacing;
    let base_z = f32(gid.y) * spacing;

    // Deterministic jitter from position hash
    let jitter = hash22(vec2<f32>(base_x, base_z));
    let world_x = params.terrain_offset.x + base_x + (jitter.x - 0.5) * spacing;
    let world_z = params.terrain_offset.z + base_z + (jitter.y - 0.5) * spacing;

    // Normalized UV for texture sampling
    let u = (world_x - params.terrain_offset.x) / params.terrain_width;
    let v = (world_z - params.terrain_offset.z) / params.terrain_depth;

    // Bounds check
    if u < 0.0 || u > 1.0 || v < 0.0 || v > 1.0 {
        return;
    }

    // Sample splat map — check density layer weight
    let splat = textureSampleLevel(splat_texture, splat_sampler, vec2<f32>(u, v), 0.0);
    var layer_weight: f32;
    switch params.density_layer {
        case 0u: { layer_weight = splat.r; }
        case 1u: { layer_weight = splat.g; }
        case 2u: { layer_weight = splat.b; }
        case 3u: { layer_weight = splat.a; }
        default: { layer_weight = splat.r; }
    }

    if layer_weight < params.density_threshold {
        return;
    }

    // Load heightmap texel for Y position (R32Float is not filterable)
    let hm_dims = textureDimensions(heightmap_texture, 0);
    let hm_coord = vec2<i32>(
        clamp(i32(u * f32(hm_dims.x)), 0, i32(hm_dims.x) - 1),
        clamp(i32(v * f32(hm_dims.y)), 0, i32(hm_dims.y) - 1),
    );
    let height_sample = textureLoad(heightmap_texture, hm_coord, 0).r;
    let world_y = params.terrain_offset.y + height_sample * params.height_scale;

    // Distance check for LOD/density falloff
    let world_pos = vec3<f32>(world_x, world_y, world_z);
    let dist = distance(world_pos, params.camera_pos);

    if dist > params.max_distance {
        return;
    }

    // Probabilistic density falloff in the fade zone
    if dist > params.fade_start {
        let fade_t = (dist - params.fade_start) / (params.max_distance - params.fade_start);
        let keep_prob = 1.0 - fade_t * fade_t; // Quadratic falloff
        let rand_val = hash21(vec2<f32>(world_x * 7.31, world_z * 13.17));
        if rand_val > keep_prob {
            return;
        }
    }

    // Generate per-blade properties from position hash
    let h1 = hash21(vec2<f32>(world_x * 3.7, world_z * 5.3));
    let h2 = hash21(vec2<f32>(world_x * 11.1, world_z * 7.9));
    let h3 = hash21(vec2<f32>(world_x * 17.3, world_z * 23.1));

    let rotation = h1 * 6.28318; // Full rotation range
    let height_scale = 1.0 + (h2 - 0.5) * 2.0 * params.height_variation;
    // Pack a simple tint variation into u32 RGBA8
    let dry_mix = h3;
    let tint_r = u32(clamp(dry_mix * 255.0, 0.0, 255.0));
    let tint = tint_r | (tint_r << 8u) | (tint_r << 16u) | (255u << 24u);

    // Write instance
    let idx = atomicAdd(&instance_count, 1u);
    if idx < arrayLength(&instances) {
        instances[idx] = GrassInstance(
            world_pos,
            rotation,
            height_scale * params.blade_height,
            tint,
        );
    }
}
