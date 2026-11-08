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
    // Workgroups in this dispatch, for the scan
    group_count: u32,
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
// A count of blades per workgroup, then (after cs_scan) its first slot
@group(2) @binding(2)
var<storage, read_write> group_slots: array<u32>;

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

// A cell's blade, if it grows one
struct Placement {
    keep: bool,
    blade: GrassInstance,
};

// Whether the grid cell `cell` (in the dispatch window) grows a blade, and
// the blade it grows. Everything about a blade comes from its place in the
// grid, so the same cell gives the same blade every frame and every run.
fn place(cell_in_window: vec2<u32>) -> Placement {
    var none: Placement;
    none.keep = false;

    // Grid spacing from density: spacing = 1 / sqrt(density)
    let spacing = 1.0 / sqrt(params.density);

    // Grid dimensions
    let grid_x = u32(ceil(params.terrain_width / spacing));
    let grid_z = u32(ceil(params.terrain_depth / spacing));

    let cell_x = cell_in_window.x + params.grid_start_x;
    let cell_z = cell_in_window.y + params.grid_start_z;
    if cell_x >= grid_x || cell_z >= grid_z {
        return none;
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
        return none;
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
        return none;
    }

    // A partial weight thins the field in proportion to it
    if params.scale_by_density != 0u {
        let keep = hash21(vec2<f32>(world_x * 5.17, world_z * 3.91));
        if keep > layer_weight {
            return none;
        }
    }

    let world_y = params.terrain_offset.y + ground_height(u, v) * params.height_scale;

    // Distance check for LOD/density falloff
    let world_pos = vec3<f32>(world_x, world_y, world_z);
    let dist = distance(world_pos, params.camera_pos);

    if dist > params.max_distance {
        return none;
    }

    // Out of view (the planes are padded, so blades that lean or cast a
    // shadow into the view stay)
    for (var i = 0u; i < 6u; i++) {
        let plane = params.frustum_planes[i];
        if dot(plane.xyz, world_pos) + plane.w < 0.0 {
            return none;
        }
    }

    // Probabilistic density falloff in the fade zone
    if dist > params.fade_start {
        let fade_t = (dist - params.fade_start) / (params.max_distance - params.fade_start);
        let keep_prob = 1.0 - fade_t * fade_t; // Quadratic falloff
        let rand_val = hash21(vec2<f32>(world_x * 7.31, world_z * 13.17));
        if rand_val > keep_prob {
            return none;
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

    return Placement(
        true,
        GrassInstance(world_pos, rotation, height_scale * params.blade_height, tint),
    );
}

// The blades are compacted into `instances` in grid order, in three passes,
// so the buffer holds the same blades in the same order every run (#341).
// An atomic append put them in whatever order the GPU's threads finished,
// and where two blades meet at one depth the one drawn first wins, so a
// capture changed by a level in a few pixels from run to run.
//
//   cs_count  each workgroup counts the blades its 8x8 cells grow
//   cs_scan   one workgroup turns the counts into each group's first slot,
//             and writes the total into the draw arguments
//   cs_emit   each workgroup places its cells again and writes its blades
//             from its first slot, in cell order

// Which cells of this workgroup grow a blade (cs_count, cs_emit)
var<workgroup> kept: array<u32, 64>;

fn group_index(group: vec3<u32>, groups: vec3<u32>) -> u32 {
    return group.x + group.y * groups.x;
}

@compute @workgroup_size(8, 8, 1)
fn cs_count(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    kept[local] = select(0u, 1u, place(gid.xy).keep);
    workgroupBarrier();
    if local == 0u {
        var count = 0u;
        for (var i = 0u; i < 64u; i++) {
            count += kept[i];
        }
        group_slots[group_index(group, groups)] = count;
    }
}

const SCAN_THREADS: u32 = 256u;
var<workgroup> partial: array<u32, 256>;

@compute @workgroup_size(256, 1, 1)
fn cs_scan(@builtin(local_invocation_index) local: u32) {
    // Each thread sums a run of the groups' counts...
    let groups = params.group_count;
    let per_thread = (groups + SCAN_THREADS - 1u) / SCAN_THREADS;
    let begin = min(local * per_thread, groups);
    let end = min(begin + per_thread, groups);
    var sum = 0u;
    for (var i = begin; i < end; i++) {
        sum += group_slots[i];
    }
    partial[local] = sum;
    workgroupBarrier();

    // ...the runs are scanned together (inclusive, Hillis-Steele)...
    for (var step = 1u; step < SCAN_THREADS; step <<= 1u) {
        var value = partial[local];
        if local >= step {
            value += partial[local - step];
        }
        workgroupBarrier();
        partial[local] = value;
        workgroupBarrier();
    }

    // ...and each thread turns its run's counts into first slots
    var slot = partial[local] - sum;
    for (var i = begin; i < end; i++) {
        let count = group_slots[i];
        group_slots[i] = slot;
        slot += count;
    }

    // The draws read the count from here, never past the buffer
    if local == SCAN_THREADS - 1u {
        atomicStore(&draw_args.instance_count, min(partial[local], arrayLength(&instances)));
    }
}

@compute @workgroup_size(8, 8, 1)
fn cs_emit(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let placement = place(gid.xy);
    kept[local] = select(0u, 1u, placement.keep);
    workgroupBarrier();
    if placement.keep {
        // This blade's place among the group's: the kept cells before it
        var rank = 0u;
        for (var i = 0u; i < local; i++) {
            rank += kept[i];
        }
        let idx = group_slots[group_index(group, groups)] + rank;
        // Full: the blades past the buffer's end are the last in grid order
        if idx < arrayLength(&instances) {
            instances[idx] = placement.blade;
        }
    }
}
