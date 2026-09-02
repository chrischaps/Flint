# Terrain Grass System — Design Spec

**Date:** 2026-03-21
**Status:** Approved
**Scope:** GPU-instanced stylized grass rendering integrated with the existing terrain system

## Overview

Add geometry-based grass to the terrain system using GPU compute placement and instanced rendering. Grass density is driven by the existing splat map (layer 0 / R channel), rendered as stylized cross-quads that complement the Kuwahara post-processing filter, and responds to entity proximity with a bend-on-contact effect.

## Requirements

- **Visual style**: Stylized/painterly cross-quad blades, not photorealistic
- **Density source**: Splat map R channel (grass layer), with density_source abstraction for future dedicated grass map
- **Interaction**: Bend-on-contact — blades push away from nearby entities
- **Wind**: Simple global sway with per-blade phase offset from position hash
- **LOD**: Density falloff — fewer instances at distance, fade to zero at max range
- **Configuration**: `[grass]` section in `.terrain.toml` and terrain component schema
- **Shadows**: Grass casts into nearest 1–2 cascaded shadow map cascades
- **Post-processing**: Outputs linear HDR; Kuwahara filter smooths alpha-tested edges naturally

## Configuration

A `[grass]` block in the terrain config. All fields have defaults so `enabled = true` is sufficient to activate.

```toml
[grass]
enabled = true
density = 8.0              # Blades per square meter
max_distance = 80.0        # Fade-out distance (meters)
fade_start = 60.0          # Distance where density falloff begins

# Blade appearance
blade_width = 0.08         # Base width (meters)
blade_height = 0.4         # Base height (meters)
height_variation = 0.3     # Random height scale ±30%
color_base = [0.15, 0.45, 0.1]    # Dark base color (RGB linear)
color_tip = [0.3, 0.7, 0.15]      # Bright tip color (RGB linear)
color_dry = [0.55, 0.5, 0.2]      # Dry/dead tint (mixed by noise)
dry_amount = 0.15          # How much dry grass to mix in (0..1)

# Wind
wind_direction = [1.0, 0.0, 0.3]  # XZ direction (normalized at runtime)
wind_speed = 1.0           # Sway frequency multiplier
wind_strength = 0.15       # Max sway displacement (meters)

# Interaction
bend_radius = 2.0          # Entity influence radius (meters)
bend_strength = 0.8        # How much blades bend (0..1)

# Density source
density_source = "splat"   # "splat" (default) or future: "map"
density_layer = 0          # Which splat layer drives density (default R=0)
density_threshold = 0.1    # Min splat weight to spawn grass
```

The schema component (`schemas/components/terrain.toml`) gets matching optional fields nested under `grass.*` with the same defaults.

## GPU Pipeline Architecture

Two-pass system: compute placement → instanced render.

### Pass 1: Compute — Grass Placement

Runs once per frame. Dispatches one thread per potential grass position on a grid over the terrain.

**Inputs (bind groups):**
- Group 0: `GrassComputeUniforms` — grass config params, camera position, time, terrain dimensions
- Group 1: Heightmap texture (R32Float) + splat map texture (RGBA8) + samplers
- Group 2: Instance storage buffer (write) + atomic counter (u32)

**Algorithm per thread:**
1. Compute potential world XZ from thread ID and terrain grid spacing (spacing = `1 / sqrt(density)`)
2. Sample splat map at normalized UV → if layer weight < `density_threshold`, skip
3. Hash(position.xz) → deterministic jitter offset (sub-grid randomization), random Y rotation, height variation, color tint
4. Sample heightmap → world Y position + terrain normal
5. Distance from camera → if > `max_distance`, skip. If > `fade_start`, probabilistic skip based on `(dist - fade_start) / (max_distance - fade_start)`
6. Atomically increment counter, write `GrassInstance` to storage buffer at that index

### Instance Storage Buffer

Pre-allocated for worst case: `density × terrain_area × coverage_estimate`.
For 256×256 terrain at 8 blades/m² with ~50% grass coverage ≈ 260K instances.

```
struct GrassInstance {  // 24 bytes, cache-friendly
    position: vec3<f32>,   // World XYZ on terrain surface
    rotation: f32,         // Y-axis rotation (radians)
    height: f32,           // Scale factor (1.0 ± height_variation)
    tint: u32,             // Packed RGBA8 color shift
}
```

Buffer size: ~260K × 24 bytes ≈ 6MB. Manageable.

### Instance Count Readback

The atomic counter is read back to CPU via a staging buffer, 1 frame behind. This avoids DrawIndirect complexity. First frame after load may draw 0 blades — imperceptible.

### Pass 2: Render — Instanced Cross-Quads

**Bind groups:**
- Group 0: Transform uniforms (shared with PBR — `view_proj`, `camera_pos`)
- Group 1: `GrassRenderUniforms` — wind params, colors, time, bend params
- Group 2: Light uniforms + shadow maps (shared with PBR)
- Group 3: Instance storage buffer (read-only) + entity positions buffer

**Draw call:** `draw_indexed(0..36, 0, 0..instance_count)` — single instanced draw for all grass.

## Grass Blade Geometry

### Cross-Quad Structure

3 intersecting quads at 60° intervals around the Y axis. Looks volumetric from any camera angle.

### Single Quad Profile

Each quad has 7 vertices across 4 segments (3 rectangular segments + pointed tip triangle):
- Base edge (2 verts) — anchored to terrain, no sway
- Segment 1 (2 verts) — slight sway
- Segment 2 (2 verts) — moderate sway
- Tip (1 vert) — maximum sway

**Per quad:** 7 vertices, 4 segments → 12 indices (4 triangles)
**Per blade:** 3 quads × 12 = 36 indices, 21 vertices total

### Vertex Layout (Shared Blade Mesh)

```
position: vec3<f32>   // Local blade space
uv: vec2<f32>         // u = 0..1 across blade, v = 0..1 base to tip
```

This mesh is created once and shared across all instances.

## Vertex Shader

### Wind Sway

Displacement increases with vertex V coordinate (v² falloff — base anchored, tip moves most). Each blade gets a phase offset from `hash(instance.position.xz)` so blades don't sway in unison.

```
displacement = wind_strength × v² × sin(time × wind_speed + phase) × wind_direction.xz
```

### Entity Bending

Entity positions are passed in a uniform buffer: `entity_count: u32` + `positions: array<vec4<f32>, 8>` (xyz = world position, w = unused/padding). Updated each frame by the CPU with the player + nearest NPCs.

For each entity in the positions buffer (max 8 entities), compute XZ distance from blade base. If within `bend_radius`, push the blade away from the entity with quadratic falloff:

```
bend = bend_strength × (1 - dist/radius)² × normalize(blade_pos - entity_pos)
```

Applied additively with wind displacement.

### Terrain Normal Alignment

Blade up-vector is interpolated between world-up and the terrain normal at the instance position. Slight lean on slopes, but not fully aligned — keeps the stylized look.

## Fragment Shader

### Alpha Cutoff

Hard alpha test (discard) against a blade silhouette shape function derived from UVs. No alpha blending — avoids sorting and order-dependent transparency. The Kuwahara post-process filter smooths the hard edges naturally.

### Lighting

Simplified PBR — directional light only (no point/spot sampling for grass):
- Sample shadow cascade at fragment world position
- Subsurface scattering approximation: when light direction opposes view direction, tips glow (backlit effect)
- No metallic component (grass is always dielectric)

### Color

```
base_color = lerp(color_base, color_tip, v)
dry_mix = noise(instance.position.xz) × dry_amount
final_color = lerp(base_color, color_dry, dry_mix) × instance.tint
```

Output: linear HDR to Rgba16Float buffer for post-processing.

## Integration

### Terrain Loading Flow

Existing flow unchanged. After terrain chunk upload, if `grass.enabled`:
1. Upload heightmap as compute-readable R32Float texture
2. Upload splat map as compute-readable RGBA8 texture (may already exist for terrain rendering)
3. Create `GrassPipeline` — compute pipeline + render pipeline + shared blade mesh + instance buffer + staging buffer
4. Pre-allocate instance storage buffer based on `density × terrain_area`

### Scene Renderer

New `GrassPipeline` struct alongside existing `TerrainPipeline`. Stored in `SceneRenderer`.

**Render order:** terrain → **grass** → opaque meshes → particles

Grass uses alpha test (not blending), so it writes depth and works correctly with SSAO and depth-tested transparent passes.

### Shared Bind Group Layouts

- Group 0: `transform_bind_group_layout` — reused from main PBR pipeline
- Group 2: `light_bind_group_layout` — reused from main PBR pipeline

No new shared layouts needed.

### Per-Frame Update

1. Write entity positions to entity buffer (player + nearby NPCs, max 8)
2. Reset atomic counter to 0 (via buffer write)
3. Dispatch compute shader
4. Read back instance count from staging buffer (1 frame behind)
5. In render pass: bind grass pipeline → `draw_indexed(0..36, 0, 0..instance_count)`

### Shadows

Grass casts shadows into cascaded shadow maps:
- Depth-only variant of grass render pipeline (same vertex shader, fragment only writes depth)
- Only render into nearest 1–2 shadow cascades (distant grass shadows aren't visible)
- Uses the same instance buffer — no additional compute dispatch

### Post-Processing

No changes needed:
- Grass outputs linear HDR to the same Rgba16Float buffer as all other geometry
- Kuwahara filter naturally stylizes grass edges
- Bloom picks up backlit grass tips if they exceed the threshold
- SSAO works because grass writes depth

### Headless Rendering

Grass renders in `flint render` (headless snapshots) identically to runtime — AI agents can validate grass visuals.

### Scripting

No new script APIs initially. `terrain_height()` already works for placement. A `grass_density_at(x, z)` API can be added later if scripts need to query grass coverage.

### Graceful Degradation

If compute shader creation fails (older hardware), log a warning and skip grass entirely — similar to the existing Kuwahara probe pattern with `catch_unwind`. The terrain renders normally without grass.

## Crate Placement

- **`flint-terrain`**: Grass config parsing, density source abstraction, position hashing utility, instance count estimation. No GPU code.
- **`flint-render`**: `GrassPipeline` struct, compute shader, vertex/fragment shaders, blade mesh generation, per-frame dispatch and draw.
- **`flint-player`**: Grass initialization in scene loading (after terrain load), per-frame entity position upload.
- **Schema**: Extend `schemas/components/terrain.toml` with `grass.*` fields.

## Testing

### Unit Tests (flint-terrain)

- Grass config parsing from TOML — defaults, overrides, validation
- Density source abstraction — splat map sampling returns correct weights for given UV
- Position hashing — deterministic, well-distributed jitter for given seed
- Instance count estimation — max buffer size calculation matches expected formula

### Integration Tests (flint-render)

- Grass pipeline creation succeeds (and gracefully skips if compute unavailable)
- Bind group layouts compatible with shared transform/light layouts
- Instance buffer round-trip: write known data → read back matches

### Visual Validation (flint render)

- Render terrain with `grass.enabled = true` → snapshot PNG → verify grass visible
- Render with `grass.enabled = false` → confirm no grass (toggle works)
- Render at different camera distances → verify density falloff

### Performance Budget

- Compute dispatch: < 0.5ms for 256×256 terrain
- Render pass: < 2ms for ~200K instances at 1080p
- Memory: ~6MB instance buffer + ~1MB blade mesh

### Not Tested Automatically

- Shader correctness — covered by visual validation via `flint render`
- Bend-on-contact — requires entity simulation, manual QA
- Wind appearance — subjective, manual tuning

## Future Extensions

These are explicitly out of scope but the design accommodates them:

- **Dedicated grass density map** — swap `density_source = "map"` and load a grayscale texture instead of sampling splat
- **Gusting wind** — add time-varying wind strength/direction; per-blade phase offset already provides the hook
- **Wind zones** — spatial wind data sampled in vertex shader
- **DrawIndirect** — replace CPU count readback with indirect draw buffer written by compute shader
- **Trample persistence** — render entity trails to a screen-space or world-space bend map, sample in vertex shader
- **Multiple grass types** — different blade meshes/colors per splat layer (`per-splat-layer config`)
- **Interaction audio** — rustling sounds triggered by entity proximity to dense grass areas
