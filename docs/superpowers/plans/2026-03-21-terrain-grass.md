# Terrain Grass System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GPU-instanced stylized grass rendering to the terrain system, driven by the existing splat map, with wind sway and entity bend-on-contact.

**Architecture:** Two-pass GPU system — a compute shader reads the heightmap + splat map to scatter grass instances into a storage buffer, then a render pass draws all instances as cross-quad blade meshes with a single instanced draw call. Configuration lives in a `[grass]` section of `.terrain.toml` and the terrain component schema.

**Tech Stack:** Rust, wgpu 23 (compute + render pipelines), WGSL shaders, bytemuck, serde

**Spec:** `docs/superpowers/specs/2026-03-21-terrain-grass-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/flint-terrain/src/grass_config.rs` | GrassConfig struct, TOML parsing, defaults, validation |
| Modify | `crates/flint-terrain/src/spec.rs` | Add `grass: Option<GrassConfig>` to TerrainSpec |
| Modify | `crates/flint-terrain/src/terrain.rs` | Add `grass` field to TerrainConfig |
| Modify | `crates/flint-terrain/src/lib.rs` | Export grass_config module |
| Modify | `schemas/components/terrain.toml` | Add grass.* fields |
| Create | `crates/flint-render/src/grass_pipeline.rs` | GrassPipeline struct, compute + render pipeline creation, blade mesh, GPU types |
| Create | `crates/flint-render/src/grass_compute.wgsl` | Compute shader — splat/heightmap sampling, instance scattering |
| Create | `crates/flint-render/src/grass_render.wgsl` | Vertex/fragment shader — wind, bending, lighting, alpha cutoff |
| Modify | `crates/flint-render/src/scene_renderer/mod.rs` | Add grass fields to SceneRenderer, load_grass(), unload_grass() |
| Modify | `crates/flint-render/src/scene_renderer/render_passes.rs` | Grass compute dispatch, grass render pass, grass shadow pass |
| Modify | `crates/flint-render/src/lib.rs` | Export grass_pipeline module, add shader parse tests |
| Modify | `crates/flint-player/src/player_app/scene_loading.rs` | Parse grass config from terrain component, pass to renderer |

---

## Chunk 1: Configuration & Schema

### Task 1: GrassConfig struct

**Files:**
- Create: `crates/flint-terrain/src/grass_config.rs`
- Modify: `crates/flint-terrain/src/lib.rs:7-24`
- Test: `crates/flint-terrain/src/grass_config.rs` (inline tests)

- [ ] **Step 1: Create grass_config.rs with GrassConfig struct and defaults**

```rust
// crates/flint-terrain/src/grass_config.rs
//! Grass configuration — parsed from `[grass]` TOML section

use serde::{Deserialize, Serialize};

/// Density source for grass placement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DensitySource {
    Splat,
    Map,
}

impl Default for DensitySource {
    fn default() -> Self {
        Self::Splat
    }
}

/// Grass rendering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrassConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_density")]
    pub density: f32,
    #[serde(default = "default_max_distance")]
    pub max_distance: f32,
    #[serde(default = "default_fade_start")]
    pub fade_start: f32,

    // Blade appearance
    #[serde(default = "default_blade_width")]
    pub blade_width: f32,
    #[serde(default = "default_blade_height")]
    pub blade_height: f32,
    #[serde(default = "default_height_variation")]
    pub height_variation: f32,
    #[serde(default = "default_color_base")]
    pub color_base: [f32; 3],
    #[serde(default = "default_color_tip")]
    pub color_tip: [f32; 3],
    #[serde(default = "default_color_dry")]
    pub color_dry: [f32; 3],
    #[serde(default = "default_dry_amount")]
    pub dry_amount: f32,

    // Wind
    #[serde(default = "default_wind_direction")]
    pub wind_direction: [f32; 3],
    #[serde(default = "default_wind_speed")]
    pub wind_speed: f32,
    #[serde(default = "default_wind_strength")]
    pub wind_strength: f32,

    // Interaction
    #[serde(default = "default_bend_radius")]
    pub bend_radius: f32,
    #[serde(default = "default_bend_strength")]
    pub bend_strength: f32,

    // Density source
    #[serde(default)]
    pub density_source: DensitySource,
    #[serde(default)]
    pub density_layer: u32,
    #[serde(default = "default_density_threshold")]
    pub density_threshold: f32,
}

impl Default for GrassConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            density: default_density(),
            max_distance: default_max_distance(),
            fade_start: default_fade_start(),
            blade_width: default_blade_width(),
            blade_height: default_blade_height(),
            height_variation: default_height_variation(),
            color_base: default_color_base(),
            color_tip: default_color_tip(),
            color_dry: default_color_dry(),
            dry_amount: default_dry_amount(),
            wind_direction: default_wind_direction(),
            wind_speed: default_wind_speed(),
            wind_strength: default_wind_strength(),
            bend_radius: default_bend_radius(),
            bend_strength: default_bend_strength(),
            density_source: DensitySource::default(),
            density_layer: 0,
            density_threshold: default_density_threshold(),
        }
    }
}

impl GrassConfig {
    /// Estimate maximum number of grass instances for buffer allocation.
    /// Assumes ~50% of terrain area is covered by grass.
    pub fn max_instances(&self, terrain_width: f32, terrain_depth: f32) -> u32 {
        let area = terrain_width * terrain_depth;
        let coverage_estimate = 0.5;
        (self.density * area * coverage_estimate).ceil() as u32
    }
}

fn default_density() -> f32 { 8.0 }
fn default_max_distance() -> f32 { 80.0 }
fn default_fade_start() -> f32 { 60.0 }
fn default_blade_width() -> f32 { 0.08 }
fn default_blade_height() -> f32 { 0.4 }
fn default_height_variation() -> f32 { 0.3 }
fn default_color_base() -> [f32; 3] { [0.15, 0.45, 0.1] }
fn default_color_tip() -> [f32; 3] { [0.3, 0.7, 0.15] }
fn default_color_dry() -> [f32; 3] { [0.55, 0.5, 0.2] }
fn default_dry_amount() -> f32 { 0.15 }
fn default_wind_direction() -> [f32; 3] { [1.0, 0.0, 0.3] }
fn default_wind_speed() -> f32 { 1.0 }
fn default_wind_strength() -> f32 { 0.15 }
fn default_bend_radius() -> f32 { 2.0 }
fn default_bend_strength() -> f32 { 0.8 }
fn default_density_threshold() -> f32 { 0.1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_grass_disabled() {
        let config = GrassConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.density, 8.0);
        assert_eq!(config.density_source, DensitySource::Splat);
    }

    #[test]
    fn parse_minimal_grass_config() {
        let toml_str = r#"enabled = true"#;
        let config: GrassConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.density, 8.0); // default
        assert_eq!(config.max_distance, 80.0); // default
    }

    #[test]
    fn parse_full_grass_config() {
        let toml_str = r#"
enabled = true
density = 12.0
max_distance = 100.0
fade_start = 70.0
blade_width = 0.1
blade_height = 0.5
height_variation = 0.4
color_base = [0.1, 0.4, 0.05]
color_tip = [0.25, 0.65, 0.1]
color_dry = [0.5, 0.45, 0.15]
dry_amount = 0.2
wind_direction = [0.5, 0.0, 1.0]
wind_speed = 1.5
wind_strength = 0.2
bend_radius = 3.0
bend_strength = 0.9
density_source = "splat"
density_layer = 0
density_threshold = 0.15
"#;
        let config: GrassConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.density, 12.0);
        assert_eq!(config.blade_height, 0.5);
        assert_eq!(config.wind_speed, 1.5);
    }

    #[test]
    fn max_instances_estimate() {
        let config = GrassConfig { density: 8.0, ..Default::default() };
        // 256x256 terrain, 8 blades/m², 50% coverage = 262144
        let max = config.max_instances(256.0, 256.0);
        assert_eq!(max, 262144);
    }

    #[test]
    fn round_trip_serialize() {
        let config = GrassConfig { enabled: true, ..Default::default() };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: GrassConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.density, config.density);
    }
}
```

- [ ] **Step 2: Add module export in lib.rs**

Add to `crates/flint-terrain/src/lib.rs` after line 12 (`pub mod spec;`):

```rust
pub mod grass_config;
```

And add to the pub use block after line 24:

```rust
pub use grass_config::GrassConfig;
```

- [ ] **Step 3: Run tests to verify**

Run: `cargo test -p flint-terrain -- grass`
Expected: All 5 grass_config tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/flint-terrain/src/grass_config.rs crates/flint-terrain/src/lib.rs
git commit -m "feat(terrain): add GrassConfig struct with TOML parsing and defaults"
```

---

### Task 2: Integrate GrassConfig into TerrainSpec and TerrainConfig

**Files:**
- Modify: `crates/flint-terrain/src/spec.rs:7-20` (TerrainSpec struct)
- Modify: `crates/flint-terrain/src/spec.rs:309-329` (default_spec method)
- Modify: `crates/flint-terrain/src/terrain.rs:7-28` (TerrainConfig struct)
- Test: `crates/flint-terrain/src/spec.rs` (existing tests + new)

- [ ] **Step 1: Add grass field to TerrainSpec**

In `crates/flint-terrain/src/spec.rs`, add import at top (after line 2):

```rust
use crate::grass_config::GrassConfig;
```

Add field to `TerrainSpec` struct (after line 19, before closing `}`):

```rust
    #[serde(default)]
    pub grass: Option<GrassConfig>,
```

In `default_spec()` method (after line 327 `splat_rules: Vec::new(),`):

```rust
            grass: None,
```

- [ ] **Step 2: Add grass field to TerrainConfig**

In `crates/flint-terrain/src/terrain.rs`, add import at top (after line 4):

```rust
use crate::grass_config::GrassConfig;
```

Add field to `TerrainConfig` struct (after line 27 `pub roughness: f32,`):

```rust
    /// Optional grass rendering configuration
    pub grass: Option<GrassConfig>,
```

- [ ] **Step 3: Fix existing tests that construct TerrainConfig**

In `crates/flint-terrain/src/lib.rs`, every `TerrainConfig { ... }` in the tests block needs `grass: None,` added. There are 4 test configs (lines 33-44, 73-84, 94-105, and any others). Add after each `roughness: 0.85,` line:

```rust
            grass: None,
```

- [ ] **Step 4: Add test for grass in terrain spec**

Add to `crates/flint-terrain/src/spec.rs` tests module (after line 431):

```rust
    #[test]
    fn parse_spec_with_grass() {
        let toml_str = r#"
[meta]
name = "grassy_hills"

[grass]
enabled = true
density = 10.0
blade_height = 0.5

[[height_layers]]
op = "noise"
"#;
        let spec: TerrainSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(spec.meta.name, "grassy_hills");
        let grass = spec.grass.unwrap();
        assert!(grass.enabled);
        assert_eq!(grass.density, 10.0);
        assert_eq!(grass.blade_height, 0.5);
        assert_eq!(grass.max_distance, 80.0); // default
    }

    #[test]
    fn spec_without_grass_has_none() {
        let spec = TerrainSpec::default_spec("no_grass");
        assert!(spec.grass.is_none());
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p flint-terrain`
Expected: All tests pass including new grass spec tests.

- [ ] **Step 6: Commit**

```bash
git add crates/flint-terrain/src/spec.rs crates/flint-terrain/src/terrain.rs crates/flint-terrain/src/lib.rs
git commit -m "feat(terrain): integrate GrassConfig into TerrainSpec and TerrainConfig"
```

---

### Task 3: Update terrain component schema

**Files:**
- Modify: `schemas/components/terrain.toml`

- [ ] **Step 1: Add grass fields to schema**

Append to `schemas/components/terrain.toml` after line 17:

```toml
# Grass rendering
"grass.enabled" = { type = "boolean", description = "Enable grass rendering", default = false }
"grass.density" = { type = "float", description = "Blades per square meter", default = 8.0 }
"grass.max_distance" = { type = "float", description = "Grass fade-out distance in meters", default = 80.0 }
"grass.fade_start" = { type = "float", description = "Distance where density falloff begins", default = 60.0 }
"grass.blade_width" = { type = "float", description = "Base blade width in meters", default = 0.08 }
"grass.blade_height" = { type = "float", description = "Base blade height in meters", default = 0.4 }
"grass.height_variation" = { type = "float", description = "Random height scale variation 0..1", default = 0.3 }
"grass.color_base" = { type = "vec3", description = "Dark base color (RGB linear)", default = [0.15, 0.45, 0.1] }
"grass.color_tip" = { type = "vec3", description = "Bright tip color (RGB linear)", default = [0.3, 0.7, 0.15] }
"grass.color_dry" = { type = "vec3", description = "Dry/dead tint (RGB linear)", default = [0.55, 0.5, 0.2] }
"grass.dry_amount" = { type = "float", description = "Dry grass mix amount 0..1", default = 0.15 }
"grass.wind_direction" = { type = "vec3", description = "Wind direction (XZ plane)", default = [1.0, 0.0, 0.3] }
"grass.wind_speed" = { type = "float", description = "Wind sway frequency multiplier", default = 1.0 }
"grass.wind_strength" = { type = "float", description = "Max sway displacement in meters", default = 0.15 }
"grass.bend_radius" = { type = "float", description = "Entity bend influence radius in meters", default = 2.0 }
"grass.bend_strength" = { type = "float", description = "Entity bend amount 0..1", default = 0.8 }
"grass.density_source" = { type = "string", description = "Density source: splat or map", default = "splat" }
"grass.density_layer" = { type = "integer", description = "Which splat layer drives grass density (0=R)", default = 0 }
"grass.density_threshold" = { type = "float", description = "Min splat weight to spawn grass 0..1", default = 0.1 }
```

- [ ] **Step 2: Verify schema loads**

Run: `cargo run --bin flint -- validate schemas`
Expected: No schema validation errors (or if this command doesn't exist, verify with `cargo build`).

- [ ] **Step 3: Commit**

```bash
git add schemas/components/terrain.toml
git commit -m "feat(schema): add grass.* fields to terrain component schema"
```

---

## Chunk 2: GPU Pipeline & Shaders

### Task 4: Grass blade mesh and GPU types

**Files:**
- Create: `crates/flint-render/src/grass_pipeline.rs`
- Modify: `crates/flint-render/src/lib.rs:8-28` (module export)

- [ ] **Step 1: Create grass_pipeline.rs with GPU types and blade mesh generation**

```rust
// crates/flint-render/src/grass_pipeline.rs
//! GPU-instanced grass rendering pipeline
//!
//! Two-pass system: compute shader places instances, render pass draws cross-quads.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Per-instance data written by compute shader, read by vertex shader.
/// 24 bytes, tightly packed.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassInstanceGpu {
    pub position: [f32; 3],  // World XYZ on terrain
    pub rotation: f32,       // Y-axis rotation (radians)
    pub height: f32,         // Scale factor (1.0 ± variation)
    pub tint: u32,           // Packed RGBA8 color shift
}

/// Uniform buffer for the compute shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassComputeUniforms {
    pub camera_pos: [f32; 3],
    pub time: f32,
    pub terrain_offset: [f32; 3],
    pub density: f32,
    pub terrain_width: f32,
    pub terrain_depth: f32,
    pub height_scale: f32,
    pub max_distance: f32,
    pub fade_start: f32,
    pub density_threshold: f32,
    pub density_layer: u32,
    pub blade_height: f32,
    pub height_variation: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

/// Uniform buffer for the render (vertex/fragment) shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassRenderUniforms {
    pub wind_direction: [f32; 3],
    pub wind_speed: f32,
    pub wind_strength: f32,
    pub time: f32,
    pub bend_radius: f32,
    pub bend_strength: f32,
    pub color_base: [f32; 3],
    pub blade_width: f32,
    pub color_tip: [f32; 3],
    pub blade_height: f32,
    pub color_dry: [f32; 3],
    pub dry_amount: f32,
    pub entity_count: u32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
    // Entity positions follow as a separate binding
}

/// Entity position for bend-on-contact (max 8).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassEntityPosition {
    pub position: [f32; 3],
    pub _pad: f32,
}

/// Maximum number of entities that can bend grass
pub const MAX_GRASS_ENTITIES: usize = 8;

/// Vertex for the shared blade quad mesh.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}

/// Number of indices in the shared blade mesh (3 quads × 4 triangles × 3 = 36)
pub const BLADE_INDEX_COUNT: u32 = 36;

/// Generate the shared cross-quad blade mesh.
/// Returns (vertices, indices) for 3 intersecting quads at 60° intervals.
/// Each quad has 7 vertices (4 segments + pointed tip).
pub fn generate_blade_mesh() -> (Vec<GrassVertex>, Vec<u16>) {
    let mut vertices = Vec::with_capacity(21);
    let mut indices = Vec::with_capacity(36);

    let half_w = 0.5_f32; // Normalized; scaled by blade_width in vertex shader

    for quad_idx in 0..3u32 {
        let angle = (quad_idx as f32) * std::f32::consts::PI / 3.0; // 0°, 60°, 120°
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        let base_vertex = (quad_idx * 7) as u16;

        // 7 vertices per quad: 2 per segment row (4 rows) minus shared tip
        // Row 0 (base): v=0.0
        // Row 1: v=0.33
        // Row 2: v=0.66
        // Row 3 (tip): v=1.0 (single vertex)
        let rows: [(f32, f32); 4] = [
            (0.0, half_w),     // base: full width
            (0.33, half_w * 0.7),
            (0.66, half_w * 0.35),
            (1.0, 0.0),       // tip: zero width (point)
        ];

        for (row_idx, &(v, hw)) in rows.iter().enumerate() {
            if row_idx < 3 {
                // Two vertices per row (left + right)
                vertices.push(GrassVertex {
                    position: [-hw * cos_a, v, -hw * sin_a],
                    uv: [0.0, v],
                });
                vertices.push(GrassVertex {
                    position: [hw * cos_a, v, hw * sin_a],
                    uv: [1.0, v],
                });
            } else {
                // Tip: single vertex
                vertices.push(GrassVertex {
                    position: [0.0, v, 0.0],
                    uv: [0.5, v],
                });
            }
        }

        // Indices: 3 rectangular segments + 1 tip triangle = 4 triangles
        // Segment 0: row0-row1
        indices.push(base_vertex);
        indices.push(base_vertex + 1);
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 1);
        indices.push(base_vertex + 3);

        // Segment 1: row1-row2
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 3);
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 3);
        indices.push(base_vertex + 5);

        // Tip triangle: row2-tip
        // Note: 2 triangles from the 2 row2 vertices to the single tip vertex
        // But since tip is a point, we get a degenerate second triangle.
        // Better: one triangle left-right-tip, skip the degenerate
        // Actually for consistent 12 indices per quad (4 tris), use both:
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 5);
        indices.push(base_vertex + 6);
        // Backface of tip (same triangle, reversed for double-sided)
        indices.push(base_vertex + 6);
        indices.push(base_vertex + 5);
        indices.push(base_vertex + 4);
    }

    (vertices, indices)
}

/// The grass rendering pipeline (compute + render)
pub struct GrassPipeline {
    pub compute_pipeline: wgpu::ComputePipeline,
    pub render_pipeline: wgpu::RenderPipeline,
    pub shadow_pipeline: wgpu::RenderPipeline,
    // Bind group layouts
    pub compute_uniform_layout: wgpu::BindGroupLayout,
    pub compute_texture_layout: wgpu::BindGroupLayout,
    pub compute_storage_layout: wgpu::BindGroupLayout,
    pub render_grass_layout: wgpu::BindGroupLayout,
    pub render_instance_layout: wgpu::BindGroupLayout,
    // Shared blade mesh
    pub blade_vertex_buffer: wgpu::Buffer,
    pub blade_index_buffer: wgpu::Buffer,
}
```

- [ ] **Step 2: Add module export to lib.rs**

In `crates/flint-render/src/lib.rs`, add after line 17 (`pub mod particle_pipeline;`):

```rust
pub mod grass_pipeline;
```

Add to the pub use block (after line 57):

```rust
pub use grass_pipeline::{
    GrassComputeUniforms, GrassEntityPosition, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, GrassVertex, BLADE_INDEX_COUNT, MAX_GRASS_ENTITIES,
};
```

- [ ] **Step 3: Run build to verify types compile**

Run: `cargo build -p flint-render`
Expected: Compiles (GrassPipeline is defined but `new()` is not yet implemented — the struct just exists).

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/grass_pipeline.rs crates/flint-render/src/lib.rs
git commit -m "feat(render): add grass GPU types, blade mesh generation, and pipeline struct"
```

---

### Task 5: Grass compute shader

**Files:**
- Create: `crates/flint-render/src/grass_compute.wgsl`

- [ ] **Step 1: Write the compute shader**

```wgsl
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

    // Sample heightmap for Y position
    let height_sample = textureSampleLevel(heightmap_texture, heightmap_sampler, vec2<f32>(u, v), 0.0).r;
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
```

- [ ] **Step 2: Add shader parse test**

In `crates/flint-render/src/lib.rs`, add to the tests module:

```rust
    #[test]
    fn grass_compute_shader_wgsl_parses() {
        let source = include_str!("grass_compute.wgsl");
        naga::front::wgsl::parse_str(source).expect("grass_compute.wgsl failed to parse");
    }
```

- [ ] **Step 3: Run parse test**

Run: `cargo test -p flint-render -- grass_compute`
Expected: PASS — shader parses without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/grass_compute.wgsl crates/flint-render/src/lib.rs
git commit -m "feat(render): add grass compute shader for instance placement"
```

---

### Task 6: Grass render shader

**Files:**
- Create: `crates/flint-render/src/grass_render.wgsl`

- [ ] **Step 1: Write the vertex/fragment shader**

The shader reads from the same `TransformUniforms` (group 0) and `LightUniforms` (group 2) as the terrain shader. Group 1 has grass-specific render uniforms. Group 3 has the instance storage buffer and entity positions.

```wgsl
// crates/flint-render/src/grass_render.wgsl
// Grass instanced rendering — stylized cross-quads with wind and bending

struct TransformUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    model_inv_transpose: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct GrassRenderUniforms {
    wind_direction: vec3<f32>,
    wind_speed: f32,
    wind_strength: f32,
    time: f32,
    bend_radius: f32,
    bend_strength: f32,
    color_base: vec3<f32>,
    blade_width: f32,
    color_tip: vec3<f32>,
    blade_height: f32,
    color_dry: vec3<f32>,
    dry_amount: f32,
    entity_count: u32,
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

struct EntityPosition {
    position: vec3<f32>,
    _pad: f32,
};

struct DirectionalLight {
    direction: vec3<f32>,
    _pad0: f32,
    color: vec3<f32>,
    intensity: f32,
    _pad1: vec3<f32>,
    _pad2: f32,
};

struct LightUniforms {
    directional: array<DirectionalLight, 4>,
    dir_count: u32,
    point_count: u32,
    spot_count: u32,
    ambient_intensity: f32,
    ambient_color: vec3<f32>,
    _pad: f32,
};

// Bind group 0: Transform (shared)
@group(0) @binding(0)
var<uniform> transform: TransformUniforms;

// Bind group 1: Grass render uniforms
@group(1) @binding(0)
var<uniform> grass: GrassRenderUniforms;

// Bind group 2: Lights (shared) — simplified, just directional + ambient
@group(2) @binding(0)
var<uniform> lights: LightUniforms;
// Shadow map bindings (group 2, bindings 1-3) declared but not sampled for grass
@group(2) @binding(1)
var shadow_depth: texture_depth_2d_array;
@group(2) @binding(2)
var shadow_sampler: sampler_comparison;

struct ShadowUniforms {
    cascade_view_proj: array<mat4x4<f32>, 3>,
    cascade_splits: vec4<f32>,
};
@group(2) @binding(3)
var<uniform> shadow_uniforms: ShadowUniforms;

// Bind group 3: Instance data + entity positions
@group(3) @binding(0)
var<storage, read> instances: array<GrassInstance>;
@group(3) @binding(1)
var<storage, read> entities: array<EntityPosition>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) world_normal: vec3<f32>,
};

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

@vertex
fn vs_main(
    vertex: VertexInput,
    @builtin(instance_index) instance_idx: u32,
) -> VertexOutput {
    let inst = instances[instance_idx];

    // Y-axis rotation matrix
    let cos_r = cos(inst.rotation);
    let sin_r = sin(inst.rotation);

    // Scale blade by instance height and uniform width
    var local_pos = vertex.position;
    local_pos.x *= grass.blade_width;
    local_pos.z *= grass.blade_width;
    local_pos.y *= inst.height;

    // Rotate around Y axis
    let rotated_x = local_pos.x * cos_r - local_pos.z * sin_r;
    let rotated_z = local_pos.x * sin_r + local_pos.z * cos_r;
    local_pos.x = rotated_x;
    local_pos.z = rotated_z;

    // Wind sway — increases with v² (tip moves most)
    let v = vertex.uv.y;
    let v_sq = v * v;
    let phase = hash21(inst.position.xz * 3.7) * 6.28318;
    let wind_offset = grass.wind_strength * v_sq * sin(grass.time * grass.wind_speed + phase);
    let wind_dir = normalize(grass.wind_direction.xz);
    local_pos.x += wind_offset * wind_dir.x;
    local_pos.z += wind_offset * wind_dir.y;

    // Entity bend-on-contact
    for (var i = 0u; i < min(grass.entity_count, 8u); i++) {
        let entity_pos = entities[i].position;
        let to_blade = inst.position.xz - entity_pos.xz;
        let dist = length(to_blade);
        if dist < grass.bend_radius && dist > 0.001 {
            let falloff = pow(1.0 - dist / grass.bend_radius, 2.0);
            let push = normalize(to_blade) * grass.bend_strength * falloff * v_sq;
            local_pos.x += push.x;
            local_pos.z += push.y;
        }
    }

    let world_pos = inst.position + local_pos;

    // Color: base-to-tip gradient with dry variation
    let base_color = mix(grass.color_base, grass.color_tip, v);
    let dry_noise = hash21(inst.position.xz * 5.3);
    let final_color = mix(base_color, grass.color_dry, dry_noise * grass.dry_amount);

    // Approximate normal (pointing mostly up, tilted by wind)
    let normal = normalize(vec3<f32>(-wind_offset * wind_dir.x * 0.3, 1.0, -wind_offset * wind_dir.y * 0.3));

    var out: VertexOutput;
    out.clip_pos = transform.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_pos = world_pos;
    out.uv = vertex.uv;
    out.color = final_color;
    out.world_normal = normal;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Alpha cutoff: blade shape from UV
    // Blade narrows toward tip (v=1). Discard outside blade silhouette.
    let u_centered = abs(in.uv.x - 0.5) * 2.0; // 0 at center, 1 at edge
    let blade_edge = 1.0 - in.uv.y * 0.7; // Edge threshold narrows with height
    if u_centered > blade_edge {
        discard;
    }

    var color = in.color;

    // Simple directional lighting
    if lights.dir_count > 0u {
        let light_dir = normalize(-lights.directional[0].direction);
        let n_dot_l = max(dot(in.world_normal, light_dir), 0.0);

        // Shadow sampling (cascade 0 only for grass)
        let shadow_pos = shadow_uniforms.cascade_view_proj[0] * vec4<f32>(in.world_pos, 1.0);
        let shadow_ndc = shadow_pos.xyz / shadow_pos.w;
        let shadow_uv = shadow_ndc.xy * vec2<f32>(0.5, -0.5) + 0.5;
        var shadow = 1.0;
        if shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0 {
            shadow = textureSampleCompare(shadow_depth, shadow_sampler, shadow_uv, 0, shadow_ndc.z - 0.002);
        }

        let diffuse = n_dot_l * lights.directional[0].intensity * shadow;

        // Subsurface scattering approximation — backlit tips glow
        let view_dir = normalize(transform.camera_pos - in.world_pos);
        let sss = pow(max(dot(view_dir, -light_dir), 0.0), 4.0) * in.uv.y * 0.3;

        let light_color = lights.directional[0].color;
        color *= (diffuse + sss) * light_color + lights.ambient_color * lights.ambient_intensity;
    } else {
        color *= lights.ambient_color * lights.ambient_intensity;
    }

    return vec4<f32>(color, 1.0);
}

// Shadow pass vertex shader — same positioning, no fragment color
@vertex
fn vs_shadow(
    vertex: VertexInput,
    @builtin(instance_index) instance_idx: u32,
) -> @builtin(position) vec4<f32> {
    let inst = instances[instance_idx];

    var local_pos = vertex.position;
    local_pos.x *= grass.blade_width;
    local_pos.z *= grass.blade_width;
    local_pos.y *= inst.height;

    let cos_r = cos(inst.rotation);
    let sin_r = sin(inst.rotation);
    let rotated_x = local_pos.x * cos_r - local_pos.z * sin_r;
    let rotated_z = local_pos.x * sin_r + local_pos.z * cos_r;
    local_pos.x = rotated_x;
    local_pos.z = rotated_z;

    // Wind (same as main pass for shadow consistency)
    let v_sq = vertex.uv.y * vertex.uv.y;
    let phase = hash21(inst.position.xz * 3.7) * 6.28318;
    let wind_offset = grass.wind_strength * v_sq * sin(grass.time * grass.wind_speed + phase);
    let wind_dir = normalize(grass.wind_direction.xz);
    local_pos.x += wind_offset * wind_dir.x;
    local_pos.z += wind_offset * wind_dir.y;

    let world_pos = inst.position + local_pos;

    // Shadow uses transform.view_proj which will be set to the shadow cascade VP
    return transform.view_proj * vec4<f32>(world_pos, 1.0);
}
```

- [ ] **Step 2: Add shader parse test**

In `crates/flint-render/src/lib.rs`, add to the tests module:

```rust
    #[test]
    fn grass_render_shader_wgsl_parses() {
        let source = include_str!("grass_render.wgsl");
        naga::front::wgsl::parse_str(source).expect("grass_render.wgsl failed to parse");
    }
```

- [ ] **Step 3: Run parse test**

Run: `cargo test -p flint-render -- grass_render`
Expected: PASS — shader parses without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/grass_render.wgsl crates/flint-render/src/lib.rs
git commit -m "feat(render): add grass vertex/fragment/shadow shaders"
```

---

### Task 7: GrassPipeline::new() — pipeline creation

**Files:**
- Modify: `crates/flint-render/src/grass_pipeline.rs`

- [ ] **Step 1: Implement GrassPipeline::new()**

Add to `crates/flint-render/src/grass_pipeline.rs` after the `GrassPipeline` struct definition:

```rust
impl GrassPipeline {
    pub fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        transform_bind_group_layout: &wgpu::BindGroupLayout,
        light_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Option<Self> {
        // Compute shader
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Grass Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_compute.wgsl").into()),
        });

        // Render shader
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Grass Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_render.wgsl").into()),
        });

        // --- Compute bind group layouts ---

        // Group 0: Compute uniforms
        let compute_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Grass Compute Uniform Layout"),
            });

        // Group 1: Heightmap + splat textures
        let compute_texture_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("Grass Compute Texture Layout"),
            });

        // Group 2: Instance storage buffer (read-write) + atomic counter
        let compute_storage_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
                label: Some("Grass Compute Storage Layout"),
            });

        // Compute pipeline
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    &compute_uniform_layout,
                    &compute_texture_layout,
                    &compute_storage_layout,
                ],
                push_constant_ranges: &[],
                label: Some("Grass Compute Pipeline Layout"),
            });

        let compute_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Grass Compute Pipeline"),
                layout: Some(&compute_pipeline_layout),
                module: &compute_shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });

        // --- Render bind group layouts ---

        // Group 1: Grass render uniforms
        let render_grass_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Grass Render Uniform Layout"),
            });

        // Group 3: Instance buffer (read) + entity positions (read)
        let render_instance_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
                label: Some("Grass Render Instance Layout"),
            });

        // Vertex buffer layout for blade mesh
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GrassVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 12,
                    shader_location: 1,
                },
            ],
        };

        // Render pipeline
        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    transform_bind_group_layout, // Group 0
                    &render_grass_layout,         // Group 1
                    light_bind_group_layout,      // Group 2
                    &render_instance_layout,      // Group 3
                ],
                push_constant_ranges: &[],
                label: Some("Grass Render Pipeline Layout"),
            });

        let render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Grass Render Pipeline"),
                layout: Some(&render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[vertex_layout.clone()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: scene_format,
                        blend: None, // Opaque with alpha test (discard in shader)
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None, // Double-sided grass blades
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        // Shadow pipeline (depth-only, uses vs_shadow entry point)
        // Uses same 4-group layout as render pipeline so the shader's @group bindings match.
        // Group 2 (lights) is bound but unused by the shadow shader — needed for layout compat.
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    transform_bind_group_layout, // Group 0: transform (VP set to cascade VP, model=identity)
                    &render_grass_layout,         // Group 1: grass uniforms (for wind)
                    light_bind_group_layout,      // Group 2: lights (unused but required for layout match)
                    &render_instance_layout,      // Group 3: instances + entities
                ],
                push_constant_ranges: &[],
                label: Some("Grass Shadow Pipeline Layout"),
            });

        let shadow_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Grass Shadow Pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_shadow"),
                    buffers: &[vertex_layout],
                    compilation_options: Default::default(),
                },
                fragment: None, // Depth-only
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 1.5,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        // Generate blade mesh
        let (blade_verts, blade_indices) = generate_blade_mesh();

        let blade_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Grass Blade Vertex Buffer"),
                contents: bytemuck::cast_slice(&blade_verts),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let blade_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Grass Blade Index Buffer"),
                contents: bytemuck::cast_slice(&blade_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        Some(Self {
            compute_pipeline,
            render_pipeline,
            shadow_pipeline,
            compute_uniform_layout,
            compute_texture_layout,
            compute_storage_layout,
            render_grass_layout,
            render_instance_layout,
            blade_vertex_buffer,
            blade_index_buffer,
        })
    }
}
```

- [ ] **Step 2: Build to verify pipeline creation compiles**

Run: `cargo build -p flint-render`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add crates/flint-render/src/grass_pipeline.rs
git commit -m "feat(render): implement GrassPipeline::new() with compute, render, and shadow pipelines"
```

---

## Chunk 3: Scene Renderer Integration

### Task 8: Add grass state to SceneRenderer

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs`

- [ ] **Step 1: Add grass fields to SceneRenderer struct**

In `crates/flint-render/src/scene_renderer/mod.rs`, add import (after line 28):

```rust
use crate::grass_pipeline::{
    GrassComputeUniforms, GrassEntityPosition, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, GrassVertex, BLADE_INDEX_COUNT, MAX_GRASS_ENTITIES,
};
```

Add fields to `SceneRenderer` struct (after line 138, `terrain_material_buffer`):

```rust
    // Grass
    grass_pipeline: Option<GrassPipeline>,
    grass_instance_buffer: Option<wgpu::Buffer>,
    grass_instance_count: u32,
    grass_max_instances: u32,
    grass_counter_buffer: Option<wgpu::Buffer>,
    grass_staging_buffer: Option<wgpu::Buffer>,
    grass_compute_uniform_buffer: Option<wgpu::Buffer>,
    grass_compute_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_compute_texture_bind_group: Option<wgpu::BindGroup>,
    grass_compute_storage_bind_group: Option<wgpu::BindGroup>,
    grass_render_uniform_buffer: Option<wgpu::Buffer>,
    grass_render_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_render_instance_bind_group: Option<wgpu::BindGroup>,
    grass_entity_buffer: Option<wgpu::Buffer>,
    grass_config: Option<flint_terrain::GrassConfig>,
    grass_terrain_offset: [f32; 3],
    grass_terrain_width: f32,
    grass_terrain_depth: f32,
    grass_terrain_height_scale: f32,
```

- [ ] **Step 2: Initialize grass pipeline in SceneRenderer::new()**

After line 209 (where `terrain_pipeline` is created), add:

```rust
        // Graceful degradation: wrap in catch_unwind like the Kuwahara pipeline.
        // If compute shaders aren't supported, grass is silently disabled.
        let grass_pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GrassPipeline::new(
                &context.device,
                scene_format,
                &pipeline.transform_bind_group_layout,
                &pipeline.light_bind_group_layout,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Grass pipeline creation failed — grass disabled");
            None
        })
        .flatten();
```

Add fields in the `Self { ... }` constructor (after `terrain_material_buffer: None,`):

```rust
            grass_pipeline,
            grass_instance_buffer: None,
            grass_instance_count: 0,
            grass_max_instances: 0,
            grass_counter_buffer: None,
            grass_staging_buffer: None,
            grass_compute_uniform_buffer: None,
            grass_compute_uniform_bind_group: None,
            grass_compute_texture_bind_group: None,
            grass_compute_storage_bind_group: None,
            grass_render_uniform_buffer: None,
            grass_render_uniform_bind_group: None,
            grass_render_instance_bind_group: None,
            grass_entity_buffer: None,
            grass_config: None,
            grass_terrain_offset: [0.0; 3],
            grass_terrain_width: 0.0,
            grass_terrain_depth: 0.0,
            grass_terrain_height_scale: 0.0,
```

- [ ] **Step 3: Add load_grass() method**

Add a new public method to `SceneRenderer` (after `unload_terrain()` method):

```rust
    /// Initialize grass GPU resources for the loaded terrain.
    /// Call after load_terrain() when grass is enabled.
    pub fn load_grass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        heightmap_data: &[f32],
        heightmap_width: u32,
        heightmap_depth: u32,
        splat_data: &[u8],     // RGBA8 splat map pixels
        splat_width: u32,
        splat_height: u32,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.max_instances(terrain_width, terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Instance storage buffer (compute writes, render reads)
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Atomic counter buffer (u32)
        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Counter Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Staging buffer for reading counter back to CPU
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Staging Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Compute uniform buffer
        let compute_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Compute Uniform Buffer"),
            size: std::mem::size_of::<GrassComputeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: compute_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Compute Uniform Bind Group"),
        });

        // Upload heightmap as R32Float texture
        let hm_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grass Heightmap Texture"),
            size: wgpu::Extent3d {
                width: heightmap_width,
                height: heightmap_depth,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload heightmap pixel data
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &hm_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(heightmap_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(heightmap_width * 4), // R32Float = 4 bytes/pixel
                rows_per_image: Some(heightmap_depth),
            },
            wgpu::Extent3d {
                width: heightmap_width,
                height: heightmap_depth,
                depth_or_array_layers: 1,
            },
        );

        // Upload splat map as RGBA8 texture
        let splat_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grass Splat Texture"),
            size: wgpu::Extent3d {
                width: splat_width,
                height: splat_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload splat pixel data
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &splat_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            splat_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(splat_width * 4), // RGBA8 = 4 bytes/pixel
                rows_per_image: Some(splat_height),
            },
            wgpu::Extent3d {
                width: splat_width,
                height: splat_height,
                depth_or_array_layers: 1,
            },
        );

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let hm_view = hm_texture.create_view(&Default::default());
        let splat_view = splat_texture.create_view(&Default::default());

        let compute_texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hm_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&linear_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&splat_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&linear_sampler) },
            ],
            label: Some("Grass Compute Texture Bind Group"),
        });

        let compute_storage_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_storage_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: counter_buffer.as_entire_binding() },
            ],
            label: Some("Grass Compute Storage Bind Group"),
        });

        // Render uniform buffer
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Render Uniform Buffer"),
            size: std::mem::size_of::<GrassRenderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_grass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: render_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Render Uniform Bind Group"),
        });

        // Entity positions buffer
        let entity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Entity Buffer"),
            size: (MAX_GRASS_ENTITIES * std::mem::size_of::<GrassEntityPosition>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: entity_buffer.as_entire_binding() },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Store everything
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_counter_buffer = Some(counter_buffer);
        self.grass_staging_buffer = Some(staging_buffer);
        self.grass_compute_uniform_buffer = Some(compute_uniform_buffer);
        self.grass_compute_uniform_bind_group = Some(compute_uniform_bind_group);
        self.grass_compute_texture_bind_group = Some(compute_texture_bind_group);
        self.grass_compute_storage_bind_group = Some(compute_storage_bind_group);
        self.grass_render_uniform_buffer = Some(render_uniform_buffer);
        self.grass_render_uniform_bind_group = Some(render_uniform_bind_group);
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_entity_buffer = Some(entity_buffer);
        self.grass_config = Some(config.clone());
        self.grass_terrain_offset = terrain_offset;
        self.grass_terrain_width = terrain_width;
        self.grass_terrain_depth = terrain_depth;
        self.grass_terrain_height_scale = height_scale;

        tracing::info!(
            "Grass loaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Clear all grass GPU resources.
    pub fn unload_grass(&mut self) {
        self.grass_instance_buffer = None;
        self.grass_instance_count = 0;
        self.grass_max_instances = 0;
        self.grass_counter_buffer = None;
        self.grass_staging_buffer = None;
        self.grass_compute_uniform_buffer = None;
        self.grass_compute_uniform_bind_group = None;
        self.grass_compute_texture_bind_group = None;
        self.grass_compute_storage_bind_group = None;
        self.grass_render_uniform_buffer = None;
        self.grass_render_uniform_bind_group = None;
        self.grass_render_instance_bind_group = None;
        self.grass_entity_buffer = None;
        self.grass_config = None;
    }

    /// Update entity positions for grass bend-on-contact.
    /// Also updates entity_count in the render uniform buffer.
    pub fn update_grass_entities(&self, queue: &wgpu::Queue, positions: &[GrassEntityPosition]) {
        let count = positions.len().min(MAX_GRASS_ENTITIES);
        if let Some(buf) = &self.grass_entity_buffer {
            if count > 0 {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&positions[..count]));
            }
        }
        // Update entity_count in the render uniform buffer (at byte offset of entity_count field)
        if let Some(render_buf) = &self.grass_render_uniform_buffer {
            let offset = std::mem::offset_of!(GrassRenderUniforms, entity_count) as u64;
            queue.write_buffer(render_buf, offset, bytemuck::cast_slice(&[count as u32]));
        }
    }
```

Note: The `load_grass` method above needs `queue` for texture uploads. Revise the signature to include `queue: &wgpu::Queue` and do the texture writes inline with `queue.write_texture()`. The implementation agent should handle this during Task 8 — the key pattern is the same as `load_terrain()` which also takes `queue`.

- [ ] **Step 4: Build to verify**

Run: `cargo build -p flint-render`
Expected: Compiles. Some warnings about unused fields are OK at this stage.

- [ ] **Step 5: Commit**

```bash
git add crates/flint-render/src/scene_renderer/mod.rs
git commit -m "feat(render): add grass state and load/unload methods to SceneRenderer"
```

---

### Task 9: Grass compute dispatch and render pass integration

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs`

- [ ] **Step 1: Add grass compute dispatch method**

Add a new method to `SceneRenderer` in `render_passes.rs`:

```rust
    /// Dispatch the grass compute shader to scatter instances.
    /// Call before render_main_pass, after update_per_frame_uniforms.
    pub fn dispatch_grass_compute(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        time: f32,
    ) {
        let config = match &self.grass_config {
            Some(c) if c.enabled => c.clone(),
            _ => return,
        };

        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let compute_uniform_bg = match &self.grass_compute_uniform_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_texture_bg = match &self.grass_compute_texture_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_storage_bg = match &self.grass_compute_storage_bind_group {
            Some(bg) => bg,
            None => return,
        };

        // Reset atomic counter to 0
        if let Some(counter_buf) = &self.grass_counter_buffer {
            queue.write_buffer(counter_buf, 0, &[0u8; 4]);
        }

        // Update compute uniforms
        let uniforms = GrassComputeUniforms {
            camera_pos: [camera.eye[0], camera.eye[1], camera.eye[2]],
            time,
            terrain_offset: self.grass_terrain_offset,
            density: config.density,
            terrain_width: self.grass_terrain_width,
            terrain_depth: self.grass_terrain_depth,
            height_scale: self.grass_terrain_height_scale,
            max_distance: config.max_distance,
            fade_start: config.fade_start,
            density_threshold: config.density_threshold,
            density_layer: config.density_layer,
            blade_height: config.blade_height,
            height_variation: config.height_variation,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };

        if let Some(buf) = &self.grass_compute_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update render uniforms (wind, colors, etc.)
        let wind_dir = {
            let d = config.wind_direction;
            let len = (d[0] * d[0] + d[2] * d[2]).sqrt().max(0.001);
            [d[0] / len, d[1], d[2] / len]
        };

        let render_uniforms = GrassRenderUniforms {
            wind_direction: wind_dir,
            wind_speed: config.wind_speed,
            wind_strength: config.wind_strength,
            time,
            bend_radius: config.bend_radius,
            bend_strength: config.bend_strength,
            color_base: config.color_base,
            blade_width: config.blade_width,
            color_tip: config.color_tip,
            blade_height: config.blade_height,
            color_dry: config.color_dry,
            dry_amount: config.dry_amount,
            entity_count: 0, // Updated by update_grass_entities
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };

        if let Some(buf) = &self.grass_render_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[render_uniforms]));
        }

        // Compute dispatch
        let spacing = 1.0 / config.density.sqrt();
        let grid_x = (self.grass_terrain_width / spacing).ceil() as u32;
        let grid_z = (self.grass_terrain_depth / spacing).ceil() as u32;
        let workgroups_x = (grid_x + 7) / 8;
        let workgroups_z = (grid_z + 7) / 8;

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Grass Compute Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&grass_pipeline.compute_pipeline);
            pass.set_bind_group(0, compute_uniform_bg, &[]);
            pass.set_bind_group(1, compute_texture_bg, &[]);
            pass.set_bind_group(2, compute_storage_bg, &[]);
            pass.dispatch_workgroups(workgroups_x, workgroups_z, 1);
        }

        // Copy counter to staging buffer for CPU readback
        if let (Some(counter), Some(staging)) =
            (&self.grass_counter_buffer, &self.grass_staging_buffer)
        {
            encoder.copy_buffer_to_buffer(counter, 0, staging, 0, 4);
        }
    }

    /// Read back the grass instance count from the staging buffer.
    /// Call after submitting the command buffer from the previous frame.
    pub fn read_grass_instance_count(&mut self, device: &wgpu::Device) {
        let staging = match &self.grass_staging_buffer {
            Some(s) => s,
            None => return,
        };

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::Maintain::Wait);

        if let Ok(Ok(())) = rx.recv() {
            let data = slice.get_mapped_range();
            let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            self.grass_instance_count = count.min(self.grass_max_instances);
            drop(data);
            staging.unmap();
        }
    }
```

- [ ] **Step 2: Add grass to render_normal_pass**

In `render_normal_pass()` (after terrain rendering at ~line 670, before outline pass):

```rust
        // Grass rendering (after terrain — also writes depth)
        if let (Some(gp), Some(render_bg), Some(instance_bg)) = (
            &self.grass_pipeline,
            &self.grass_render_uniform_bind_group,
            &self.grass_render_instance_bind_group,
        ) {
            if self.grass_instance_count > 0 {
                render_pass.set_pipeline(&gp.render_pipeline);
                render_pass.set_bind_group(1, render_bg, &[]);
                render_pass.set_bind_group(2, &self.light_bind_group, &[]);
                render_pass.set_bind_group(3, instance_bg, &[]);
                render_pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    gp.blade_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                // Group 0 (transform) is already set from terrain pass
                render_pass.draw_indexed(
                    0..BLADE_INDEX_COUNT,
                    0,
                    0..self.grass_instance_count,
                );
            }
        }
```

- [ ] **Step 3: Add grass to shadow pass**

In `render_shadow_pass()` (after terrain shadow rendering at ~line 134, before transparent):

```rust
                // Render grass into shadow cascade (nearest 2 only)
                if cascade_idx < 2 {
                    if let (Some(gp), Some(render_bg), Some(instance_bg)) = (
                        &self.grass_pipeline,
                        &self.grass_render_uniform_bind_group,
                        &self.grass_render_instance_bind_group,
                    ) {
                        if self.grass_instance_count > 0 {
                            // Create shadow VP bind group for grass
                            let shadow_uniforms = ShadowDrawUniforms {
                                light_view_proj: cascade_vp,
                                model: identity_matrix(), // Grass positions are already world-space
                            };
                            let shadow_buffer = device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("Grass Shadow Uniform"),
                                    contents: bytemuck::cast_slice(&[shadow_uniforms]),
                                    usage: wgpu::BufferUsages::UNIFORM,
                                },
                            );
                            let shadow_bind =
                                device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    layout: &shadow_pass.shadow_bind_group_layout,
                                    entries: &[wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: shadow_buffer.as_entire_binding(),
                                    }],
                                    label: Some("Grass Shadow Bind Group"),
                                });

                            pass.set_pipeline(&gp.shadow_pipeline);
                            pass.set_bind_group(0, &shadow_bind, &[]);
                            pass.set_bind_group(1, render_bg, &[]);
                            pass.set_bind_group(2, &self.light_bind_group, &[]); // unused but needed for layout compat
                            pass.set_bind_group(3, instance_bg, &[]);
                            pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                gp.blade_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.draw_indexed(
                                0..BLADE_INDEX_COUNT,
                                0,
                                0..self.grass_instance_count,
                            );
                        }
                    }
                }
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p flint-render`
Expected: Compiles. Note: `Camera` type needs to be accessible — check it's imported. The `identity_matrix` helper is already imported from `helpers`.

- [ ] **Step 5: Commit**

```bash
git add crates/flint-render/src/scene_renderer/render_passes.rs
git commit -m "feat(render): integrate grass compute dispatch, render pass, and shadow pass"
```

---

## Chunk 4: Player Integration & Validation

### Task 10: Scene loading integration

**Files:**
- Modify: `crates/flint-player/src/player_app/scene_loading.rs:60-200`

- [ ] **Step 1: Parse grass config from terrain component and pass to renderer**

In `load_terrain_from_world_inner()`, after the `scene_renderer.load_terrain(...)` call (~line 183), add:

```rust
        // Load grass if enabled
        let grass_config = {
            let mut gc = flint_terrain::GrassConfig::default();
            if let Some(enabled) = terrain_comp.get("grass.enabled") {
                if enabled.as_bool().unwrap_or(false) {
                    gc.enabled = true;
                    gc.density = get_f32("grass.density", gc.density);
                    gc.max_distance = get_f32("grass.max_distance", gc.max_distance);
                    gc.fade_start = get_f32("grass.fade_start", gc.fade_start);
                    gc.blade_width = get_f32("grass.blade_width", gc.blade_width);
                    gc.blade_height = get_f32("grass.blade_height", gc.blade_height);
                    gc.height_variation = get_f32("grass.height_variation", gc.height_variation);
                    gc.wind_speed = get_f32("grass.wind_speed", gc.wind_speed);
                    gc.wind_strength = get_f32("grass.wind_strength", gc.wind_strength);
                    gc.bend_radius = get_f32("grass.bend_radius", gc.bend_radius);
                    gc.bend_strength = get_f32("grass.bend_strength", gc.bend_strength);
                    gc.density_threshold = get_f32("grass.density_threshold", gc.density_threshold);
                    gc.density_layer = get_i32("grass.density_layer", gc.density_layer as i32) as u32;
                    gc.dry_amount = get_f32("grass.dry_amount", gc.dry_amount);

                    // Parse vec3 fields
                    if let Some(v) = terrain_comp.get("grass.color_base").and_then(toml_vec3) {
                        gc.color_base = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.color_tip").and_then(toml_vec3) {
                        gc.color_tip = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.color_dry").and_then(toml_vec3) {
                        gc.color_dry = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.wind_direction").and_then(toml_vec3) {
                        gc.wind_direction = v;
                    }
                }
            }
            gc
        };

        if grass_config.enabled {
            // Get raw heightmap data and splat map data for GPU upload
            let hm_data = heightmap.clone_heights();
            let hm_w = heightmap.width;
            let hm_d = heightmap.depth;

            // Load splat map pixels for grass compute shader
            let splat_path = {
                let p = scene_dir.join(&config.splat_map_path);
                if p.exists() { p } else {
                    scene_dir.parent().map(|pp| pp.join(&config.splat_map_path))
                        .filter(|pp| pp.exists())
                        .unwrap_or(p)
                }
            };

            if let Ok(splat_img) = image::open(&splat_path) {
                let splat_rgba = splat_img.to_rgba8();
                let (sw, sh) = splat_rgba.dimensions();

                scene_renderer.load_grass(
                    device,
                    queue,
                    &grass_config,
                    &hm_data,
                    hm_w,
                    hm_d,
                    &splat_rgba,
                    sw,
                    sh,
                    offset,
                    config.width,
                    config.depth,
                    config.height_scale,
                );

                tracing::info!("Grass enabled: density={}, max_dist={}", grass_config.density, grass_config.max_distance);
            } else {
                tracing::warn!("Grass enabled but splat map not found at {:?}", splat_path);
            }
        }
```

- [ ] **Step 2: Build to verify**

Run: `cargo build -p flint-player`
Expected: Compiles. The `image` crate should already be a dependency of flint-player (used for screenshot/texture loading).

- [ ] **Step 3: Commit**

```bash
git add crates/flint-player/src/player_app/scene_loading.rs
git commit -m "feat(player): parse grass config from terrain component and initialize grass rendering"
```

---

### Task 11: Game loop integration

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs` (render_to method)
- Modify: `crates/flint-player/src/player_app/mod.rs` (entity position updates)

The grass compute dispatch and instance count readback belong inside `render_to()` in the scene renderer — not in the player — because they need the `CommandEncoder` and must happen relative to `queue.submit()`. The player is only responsible for updating entity positions before rendering.

- [ ] **Step 1: Find render_to() in scene_renderer**

Search for the `render_to` (or `render`) method in `crates/flint-render/src/scene_renderer/mod.rs`. This is where `update_per_frame_uniforms`, `render_shadow_pass`, and `render_main_pass` are called with the `CommandEncoder`.

- [ ] **Step 2: Add grass compute dispatch inside render_to()**

Inside `render_to()`, after `update_per_frame_uniforms` and before `render_shadow_pass`:

```rust
// Dispatch grass compute to scatter instances
self.dispatch_grass_compute(device, queue, &mut encoder, camera, time);
```

The `time` value should come from the same game clock source used for particles/animation. The implementation agent should find the existing time parameter.

- [ ] **Step 3: Add instance count readback after submit**

After `queue.submit()` inside `render_to()`:

```rust
// Read back grass instance count for next frame's draw call
self.read_grass_instance_count(device);
```

- [ ] **Step 4: Add entity position updates in the player**

In the player's frame update (before calling `scene_renderer.render()`), update entity positions for bend-on-contact:

```rust
// Update grass entity positions for bend-on-contact
let mut grass_entities = Vec::new();
if let Some(player_pos) = get_player_position(world) {
    grass_entities.push(GrassEntityPosition {
        position: [player_pos.x, player_pos.y, player_pos.z],
        _pad: 0.0,
    });
}
scene_renderer.update_grass_entities(queue, &grass_entities);
```

The implementation agent should follow existing patterns for how `update_particles` and similar per-frame systems are called, and find the right location for `update_grass_entities`.

- [ ] **Step 5: Build and test**

Run: `cargo build -p flint-player`
Expected: Compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/flint-render/src/scene_renderer/mod.rs crates/flint-player/src/player_app/
git commit -m "feat(render,player): wire up grass compute dispatch in render loop and entity bending"
```

---

### Task 12: Visual validation

**Files:**
- Modify: `demo/terrain_test.scene.toml` (add grass config for testing)

- [ ] **Step 1: Add grass config to test scene**

Append grass fields to the terrain entity in `demo/terrain_test.scene.toml`:

```toml
"grass.enabled" = true
"grass.density" = 6.0
"grass.blade_height" = 0.35
"grass.max_distance" = 60.0
```

- [ ] **Step 2: Render test snapshot**

Run:

```bash
cargo run --bin flint -- render demo/terrain_test.scene.toml \
    --output /tmp/grass_test.png --schemas schemas \
    --width 1280 --height 720 \
    --distance 15 --pitch 20 --yaw 45 --target 0,1,0
```

Expected: The render produces a PNG showing terrain with visible grass blades on the green (layer 0) areas. Open the image to verify grass is present.

- [ ] **Step 3: Render without grass for comparison**

Temporarily remove grass fields, render again, compare. The grass-enabled version should show visible blade geometry on grass-textured areas.

- [ ] **Step 4: Commit test scene**

```bash
git add demo/terrain_test.scene.toml
git commit -m "feat(demo): enable grass in terrain test scene for visual validation"
```

---

## Implementation Notes

**Order matters:** Tasks 1-3 (config) have no GPU dependency and can be done first. Tasks 4-7 (shaders/pipeline) depend on the config types. Tasks 8-9 (scene renderer) depend on the pipeline. Tasks 10-12 (player integration) depend on everything above.

**Shader iteration:** The WGSL shaders (Tasks 5-6) will likely need iteration during visual validation (Task 12). The naga parse tests catch syntax errors but not visual correctness — expect to tune wind strength, alpha cutoff thresholds, and lighting parameters.

**Graceful degradation:** If `GrassPipeline::new()` returns `None` (compute shaders unsupported), all downstream code already handles `grass_pipeline: None` by early-returning. Consider wrapping the `new()` call in `catch_unwind` like the Kuwahara pipeline does (see `crates/flint-render/src/scene_renderer/mod.rs` around line 204).

**`load_grass` queue parameter:** `load_grass` accepts `queue: &wgpu::Queue` and performs `queue.write_texture()` calls to upload heightmap/splat data inline during initialization.
