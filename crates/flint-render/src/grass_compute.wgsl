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
    scale_by_density: u32,
    // First cell of the dispatch window; the dispatch covers only the cells
    // near the camera, but each keeps its place in the whole grid (ADR 0089).
    grid_start_x: u32,
    grid_start_z: u32,
    // 1: the heightmap's samples sit on the grid's corners, the first at the
    // origin and the last at the far edge, as a terrain's mesh reads them.
    // 0: each sits in the middle of its texel's cell, as a game's ground
    // map does (ADR 0098)
    height_at_corners: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // The camera frustum, each plane pushed out for blade height and shadow
    // reach; a blade rooted outside any plane isn't drawn (ADR 0092)
    frustum_planes: array<vec4<f32>, 6>,
};

// DrawIndexedIndirect arguments: the draws read the count from here, so the
// CPU never waits for it (ADR 0092)
struct DrawArgs {
    index_count: u32,
    instance_count: atomic<u32>,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
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

// Group 1: Ground textures: a terrain's heightmap and splat map, or a game's
// heightmap (world y, scale 1) and R8 density mask (ADR 0089)
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
var<storage, read_write> draw_args: DrawArgs;

// Hash function for deterministic pseudo-random values from position
fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Sinless like hash21 above — GPU sin() loses accuracy at the large
// coordinates a big terrain produces, so fract(sin(...)) is unreliable.
fn hash22(p: vec2<f32>) -> vec2<f32> {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * vec3<f32>(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((vec2<f32>(p3.x, p3.x) + vec2<f32>(p3.y, p3.z)) * vec2<f32>(p3.z, p3.y));
}

// The ground's height at (u, v), blended from the four samples round it
// (ADR 0098). R32Float isn't filterable without a device feature, so the
// blend is done here from four loads. Past the outermost samples the
// height holds level.
fn ground_height(u: f32, v: f32) -> f32 {
    let dims = vec2<i32>(textureDimensions(heightmap_texture, 0));
    let last = dims - vec2<i32>(1, 1);
    var at: vec2<f32>;
    if params.height_at_corners != 0u {
        at = vec2<f32>(u, v) * vec2<f32>(last);
    } else {
        at = vec2<f32>(u, v) * vec2<f32>(dims) - 0.5;
    }
    at = clamp(at, vec2<f32>(0.0), vec2<f32>(last));
    let i0 = vec2<i32>(floor(at));
    let i1 = min(i0 + vec2<i32>(1, 1), last);
    let t = at - vec2<f32>(i0);
    let h00 = textureLoad(heightmap_texture, i0, 0).r;
    let h10 = textureLoad(heightmap_texture, vec2<i32>(i1.x, i0.y), 0).r;
    let h01 = textureLoad(heightmap_texture, vec2<i32>(i0.x, i1.y), 0).r;
    let h11 = textureLoad(heightmap_texture, i1, 0).r;
    return mix(mix(h00, h10, t.x), mix(h01, h11, t.x), t.y);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    // Grid spacing from density: spacing = 1 / sqrt(density)
    let spacing = 1.0 / sqrt(params.density);

    // Grid dimensions
    let grid_x = u32(ceil(params.terrain_width / spacing));
    let grid_z = u32(ceil(params.terrain_depth / spacing));

    let cell_x = gid.x + params.grid_start_x;
    let cell_z = gid.y + params.grid_start_z;
    if cell_x >= grid_x || cell_z >= grid_z {
        return;
    }

    // Base position on grid
    let base_x = f32(cell_x) * spacing;
    let base_z = f32(cell_z) * spacing;

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

    // A partial weight thins the field in proportion to it
    if params.scale_by_density != 0u {
        let keep = hash21(vec2<f32>(world_x * 5.17, world_z * 3.91));
        if keep > layer_weight {
            return;
        }
    }

    let world_y = params.terrain_offset.y + ground_height(u, v) * params.height_scale;

    // Distance check for LOD/density falloff
    let world_pos = vec3<f32>(world_x, world_y, world_z);
    let dist = distance(world_pos, params.camera_pos);

    if dist > params.max_distance {
        return;
    }

    // Out of view (the planes are padded, so blades that lean or cast a
    // shadow into the view stay)
    for (var i = 0u; i < 6u; i++) {
        let plane = params.frustum_planes[i];
        if dot(plane.xyz, world_pos) + plane.w < 0.0 {
            return;
        }
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
    let idx = atomicAdd(&draw_args.instance_count, 1u);
    if idx < arrayLength(&instances) {
        instances[idx] = GrassInstance(
            world_pos,
            rotation,
            height_scale * params.blade_height,
            tint,
        );
    } else {
        // Full: give the slot back, so the count the draws read never runs
        // past the buffer. It can't drop below the buffer's length, since
        // only a blade that found it full takes one back.
        atomicSub(&draw_args.instance_count, 1u);
    }
}
