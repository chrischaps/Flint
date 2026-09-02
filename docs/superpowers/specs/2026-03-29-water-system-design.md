# Water System Design

## Overview

Add ocean water support to Flint: an infinite ocean plane with Gerstner wave simulation, stylized rendering (Fresnel, depth fog, foam), buoyancy physics for rideable boats/rafts, swimming and underwater exploration, and a showcase scene demonstrating all features.

## Architecture: Hybrid Crate Model

A new `flint-water` crate owns the core wave simulation and query API. Other crates consume its outputs:

| Crate | Responsibility |
|-------|---------------|
| `flint-water` (new) | Wave math, `WaterConfig`, `WaterState`, query API. Depends only on `flint-core`. |
| `flint-render` | New `WaterPipeline` + `water_shader.wgsl`, underwater post-process mode. Does NOT depend on `flint-water` — wave params uploaded as uniforms, GPU evaluates independently. |
| `flint-physics` | Buoyancy force calculation using water query API, drag forces. Depends on `flint-water`. |
| `flint-player` | Swim controller, player water state machine, vessel boarding, underwater camera trigger. Depends on `flint-water`. |
| `flint-script` | Water query functions, vessel API, water state callbacks. Depends on `flint-water`. |
| `flint-scene` | Parse `[water]` TOML block via `WaterDef`/`WaveLayerDef` structs. Does NOT depend on `flint-water` — `WaterConfig` is constructed from `WaterDef` at load time in `flint-player`. |
| `flint-audio` | No changes — scripts handle water audio via existing system. |
| `schemas/` | `buoyant` component, `vessel` component, `vessel` archetype. |

### Cargo Workspace Changes

Add `flint-water` to `Cargo.toml` workspace members. New dependency edges:

- `flint-water` → `flint-core`
- `flint-physics` → `flint-water`
- `flint-player` → `flint-water`
- `flint-script` → `flint-water`
- `flint-render` → (no `flint-water` dependency)
- `flint-scene` → (no `flint-water` dependency)

This mirrors how `flint-terrain` works: the terrain crate owns heightmap data and chunk generation with zero dependency on `flint-render`. The renderer consumes terrain data through a clean interface.

## 1. Wave Simulation (`flint-water`)

### Wave Model: Gerstner Waves

Gerstner waves produce convincing circular orbital motion (sharper crests than troughs) without FFT complexity. Multiple waves are summed for natural-looking results. The system supports 4–8 wave layers.

Each wave layer has these parameters:

- `amplitude: f32` — wave height
- `wavelength: f32` — crest-to-crest distance
- `speed: f32` — phase velocity
- `direction: Vec2` — propagation direction (normalized)
- `steepness: f32` — 0.0–1.0, sharpness of crests

### WaterConfig

Loaded from a `[water]` block in scene TOML (scene-level, like `[camera]` and `[post_process]`):

```toml
[water]
enabled = true
water_level = 0.0
inverse_solve_iterations = 3    # iteration count for height_at() inverse Gerstner solve

# Visual properties
shallow_color = [0.1, 0.4, 0.35, 0.85]
deep_color = [0.02, 0.08, 0.12, 0.95]
foam_color = [0.9, 0.95, 0.95, 0.8]
depth_fade = 8.0
foam_threshold = 0.6
fresnel_power = 5.0
normal_map = ""             # optional path, built-in procedural default if empty
foam_texture = ""           # optional path, built-in procedural default if empty

# Wave layers
[[water.waves]]
amplitude = 1.2
wavelength = 40.0
speed = 3.0
direction = [1.0, 0.3]
steepness = 0.4

[[water.waves]]
amplitude = 0.5
wavelength = 15.0
speed = 2.0
direction = [0.6, 1.0]
steepness = 0.3

[[water.waves]]
amplitude = 0.15
wavelength = 5.0
speed = 1.5
direction = [-0.3, 0.8]
steepness = 0.5
```

### Scene Integration: `WaterDef` Structs

`flint-scene` defines typed serde structs for the `[water]` block (like `CameraDef`, `PostProcessDef`):

```rust
// In flint-scene/src/format.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterDef {
    pub enabled: Option<bool>,              // default: true
    pub water_level: Option<f32>,           // default: 0.0
    pub inverse_solve_iterations: Option<u32>, // default: 3
    pub shallow_color: Option<[f32; 4]>,
    pub deep_color: Option<[f32; 4]>,
    pub foam_color: Option<[f32; 4]>,
    pub depth_fade: Option<f32>,
    pub foam_threshold: Option<f32>,
    pub fresnel_power: Option<f32>,
    pub normal_map: Option<String>,
    pub foam_texture: Option<String>,
    pub waves: Option<Vec<WaveLayerDef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveLayerDef {
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub direction: [f32; 2],
    pub steepness: f32,
}

// Add to SceneFile:
pub struct SceneFile {
    pub scene: SceneMetadata,
    pub camera: Option<CameraDef>,
    pub environment: Option<EnvironmentDef>,
    pub post_process: Option<PostProcessDef>,
    pub water: Option<WaterDef>,           // NEW
    // ...
}
```

`flint-player` converts `WaterDef` → `WaterConfig` (from `flint-water`) at scene load time, applying defaults.

### Query API

`flint-water` exposes pure functions with no render or ECS dependency — just math on `flint-core` types.

```rust
pub struct WaterConfig {
    pub enabled: bool,
    pub water_level: f32,
    pub inverse_solve_iterations: u32,
    pub shallow_color: [f32; 4],
    pub deep_color: [f32; 4],
    pub foam_color: [f32; 4],
    pub depth_fade: f32,
    pub foam_threshold: f32,
    pub fresnel_power: f32,
    pub normal_map: Option<String>,
    pub foam_texture: Option<String>,
    pub waves: Vec<WaveLayer>,
}

pub struct WaveLayer {
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub direction: [f32; 2],
    pub steepness: f32,
}

pub struct WaterState {
    config: WaterConfig,
}

impl WaterState {
    pub fn new(config: WaterConfig) -> Self;

    /// Sample displaced position at (x, z) for given time
    pub fn surface_point(&self, x: f32, z: f32, time: f32) -> Vec3;

    /// Surface height (Y) at world (x, z) — accounts for
    /// horizontal Gerstner displacement via iterative solve
    /// (iteration count from config.inverse_solve_iterations, default 3)
    pub fn height_at(&self, x: f32, z: f32, time: f32) -> f32;

    /// Surface normal at (x, z) — for physics and shading
    pub fn normal_at(&self, x: f32, z: f32, time: f32) -> Vec3;

    /// Wave velocity at point — for current/drift forces
    pub fn velocity_at(&self, x: f32, z: f32, time: f32) -> Vec3;

    /// Foam intensity 0.0–1.0 (based on Jacobian/steepness)
    pub fn foam_at(&self, x: f32, z: f32, time: f32) -> f32;

    /// Is this point underwater?
    pub fn is_submerged(&self, pos: Vec3, time: f32) -> bool;

    /// Submersion depth (negative = above water)
    pub fn submersion_depth(&self, pos: Vec3, time: f32) -> f32;

    /// Access config for uploading to GPU uniforms
    pub fn config(&self) -> &WaterConfig;
}
```

### Key Decision: CPU + GPU Sync

The same Gerstner math runs on CPU (for physics/buoyancy) and GPU (for rendering). Parameters are shared, not results. The wave layer data is uploaded as a uniform buffer to the water shader and evaluated identically on both sides.

### Performance Note

With 5+ buoyant entities each sampling 5 points at 3 iterations each, that is ~75 inverse solves per physics tick (~225 wave evaluations). This is lightweight for CPU. The `inverse_solve_iterations` config field allows tuning if needed for scenes with many buoyant objects.

## 2. Water Rendering (`flint-render`)

### Projected Grid

Instead of a massive world-space mesh, the water uses a screen-space projected grid. A grid is projected from camera space onto the water plane, then vertices are displaced by Gerstner waves in the vertex shader.

- Resolution automatically adapts to camera distance — dense near camera, sparse at horizon
- No LOD system needed — the grid IS the LOD
- Infinite extent with fixed vertex count (~64×64 to ~128×128 grid)
- Works perfectly for the infinite plane model

### Pipeline Architecture

```
Group 0: WaterUniforms
  ├── view_proj, camera_pos, time
  ├── water_level, grid_size, grid_extent
  ├── inv_view_proj (for grid projection)
  └── array<WaveLayer, 8> + wave_count

Group 1: WaterMaterialUniforms + Textures
  ├── shallow_color, deep_color, foam_color
  ├── depth_fade, foam_threshold, fresnel_power
  ├── normal_map texture + sampler (tiling detail normals)
  ├── foam_texture + sampler (stylized foam pattern)
  └── scene_depth texture + sampler (copy of depth buffer for shore foam)

Group 2: LightUniforms + Shadows (shared with PBR/terrain)

Group 3: unused
```

Each `WaveLayer` GPU struct: direction (vec2f) + amplitude (f32) + wavelength (f32) + speed (f32) + steepness (f32) + padding (vec2f) = 32 bytes. 8 layers = 256 bytes — well within uniform buffer limits.

### Texture Assets

Normal map and foam textures use built-in procedural defaults generated at init (simple tileable noise patterns). Optional TOML paths override with disk-loaded textures. This follows Flint's "sensible defaults with optional overrides" philosophy.

### Depth Buffer Access for Shore Foam

Before the water render pass, the scene depth buffer is copied to a separate read-only depth texture. This copy is bound in Group 1 alongside material textures. The water fragment shader samples this to compute shore proximity (shallow depth = more foam). This is a new pattern — unlike SSAO (which reads the depth texture in its own separate post-process pass), water needs a depth copy because it both reads scene depth and writes its own depth within the main render pass sequence.

### Shader Technique

**Vertex shader:**
- Project grid onto water plane using `inv_view_proj`
- Sum Gerstner displacement per vertex
- Compute tangent/bitangent for normals
- Pass world position + foam factor to fragment

**Fragment shader:**
- **Fresnel**: Schlick approximation — more reflection at glancing angles (skybox reflection, not planar mirror)
- **Depth fog**: Blend shallow→deep color based on water depth (scene depth - water depth)
- **Detail normals**: Scrolling normal map (2 layers, different speeds) for surface detail
- **Foam**: Wave crest steepness + shore proximity (depth-based, using scene_depth texture)
- **Specular**: Simplified Blinn-Phong sun highlight (stylized, not full PBR)

### Render Pass Integration

Water renders after all opaques (terrain, grass, entities) so the depth buffer is fully populated for shore foam. Water is effectively a transparent surface and renders before other transparents:

```
1. Shadow Pass
2. Main Pass:
   a. Skybox
   b. Grid (debug)
   c. Terrain
   d. Grass
   e. Outlines
   f. Entities (opaque)
   g. Skinned Entities
   h. Billboards
   i. ── Depth buffer copy (new) ──
   j. ── Water (new) ──
   k. Transparent entities
   l. Particles
   m. 2D Sprites / UI
3. Post-Processing (with underwater mode)
```

Water writes to both color and depth buffers. Alpha blending enabled for transparency. The depth buffer copy happens between opaques and water so the water shader can read scene depth for shore foam while also writing its own depth for correct sorting with transparent entities behind it.

**Implementation note: render pass split.** The current main pass is a single `begin_render_pass`/`end_render_pass` block. A `copy_texture_to_texture` (encoder command) cannot happen mid-render-pass. Implementation must split the main pass into two sub-passes: (1) opaques pass (skybox through billboards), then end pass, issue depth copy, (2) water + transparents pass. This is a straightforward refactor — the draw calls are already ordered, they just need to be split across two `begin_render_pass` calls with the same color/depth attachments.

### Underwater Rendering

When the camera is below the water surface, underwater effects are handled through the existing post-processing pipeline (not a separate in-pass fullscreen quad), avoiding the wgpu limitation of reading and writing the same depth buffer simultaneously:

- **Water surface rendered from below** — same water shader, but flip the normal. Lighter color from underneath.
- **Underwater fog** — extend the existing fog pass in `PostProcessPipeline` with an underwater mode: when camera is submerged, swap fog color to `deep_color`, increase density, reduce visibility range. The composite shader already reads the depth buffer in a separate pass, so no depth read/write conflict.
- **Underwater tint** — post-process color shift applied in the composite pass (simple multiply with underwater color based on `is_camera_submerged` uniform).
- **Wavy distortion** — optional: reuse existing composite shader's chromatic aberration path with time-varying sinusoidal offset for subtle underwater wobble.

### Headless Rendering

`WaterPipeline` is initialized in `HeadlessContext` (alongside existing terrain, PBR pipelines) so `flint render` works with water scenes. Water uniform data is uploaded the same way as in windowed mode.

### `flint edit` Integration

Water renders automatically in the interactive scene viewer when a scene has a `[water]` block, since `SceneRenderer` handles all pipelines. No special `flint edit` code needed — it follows the same path as terrain.

### What We Skip (Keeping It Stylized)

- **No planar reflections** — no second render pass mirroring the scene. Skybox reflection via Fresnel instead.
- **No screen-space refraction** — depth-based color blending gives the underwater look without distortion passes.
- **No caustics** — could be added later as a projected texture.
- **No tessellation** — projected grid gives adaptive detail without hardware tessellation (wgpu doesn't support it).

## 3. Buoyancy System (`flint-physics`)

Buoyancy is computed per-frame for entities with a `buoyant` component using multi-point sampling for realistic tilt/roll.

### Algorithm

For each buoyant entity, each physics tick:

1. Sample N buoyancy points (local-space offsets on the hull). Boat: 4 corners + center = 5 points. Simple object: 1 center point.
2. For each point: transform to world space, query `water.height_at(x, z, time)`, compute submersion depth. If submerged: apply upward force proportional to submersion × point weight.
3. Apply water drag (linear + angular damping) — prevents infinite bobbing, settles naturally.
4. Apply current force from `water.velocity_at()` — drift with waves.

### Physics Integration

`PhysicsSystem` gains a new method `apply_buoyancy(&mut self, world: &mut FlintWorld, water: &WaterState, time: f32)`, called by `PlayerApp` after `fixed_update()`. This matches the existing pattern where `update_character()` is a separate method called from `PlayerApp` rather than being part of the `RuntimeSystem::fixed_update` trait method. The `WaterState` is owned by `PlayerApp` and passed in, keeping the `RuntimeSystem` trait signature unchanged.

### Component Schema

Following the pattern used by physics-adjacent components (`collider`, `rigidbody`, `character_controller`) which use fields directly under the component table (no `.fields` sub-table):

```toml
# schemas/components/buoyant.toml
[component.buoyant]
description = "Makes a rigidbody float on water"
buoyancy_force = { type = "float", default = 10.0 }
drag = { type = "float", default = 1.5 }
angular_drag = { type = "float", default = 2.0 }
```

Note: `sample_points` and `sample_weights` are not schema fields. Instead, if the entity has a `bounds` component, buoyancy auto-generates sample points from the AABB corners + center. For custom sample points, use a script in `on_init` to configure them via a new `set_buoyancy_points(entity_id, points)` script function. This avoids introducing new array schema types.

## 4. Vessel System

A `vessel` component marks an entity as a rideable watercraft. Works with the existing `interactable` component for boarding.

### Component Schema

```toml
# schemas/components/vessel.toml
[component.vessel]
description = "Rideable watercraft"
seat_offset = { type = "vec3", default = [0, 0.5, 0] }
throttle_force = { type = "float", default = 15.0 }
turn_torque = { type = "float", default = 8.0 }
max_speed = { type = "float", default = 12.0 }
camera_offset = { type = "vec3", default = [0, 3, -8] }
```

### Archetype

```toml
# schemas/archetypes/vessel.toml
[archetype.vessel]
description = "Rideable watercraft with buoyancy"
components = ["transform", "rigidbody", "collider", "buoyant", "vessel", "interactable"]

[archetype.vessel.defaults.buoyant]
buoyancy_force = 12.0
drag = 1.5

[archetype.vessel.defaults.interactable]
prompt_text = "Board"
range = 3.0
```

Note: `model` is not a schema component — models are loaded via the `model` field directly in entity TOML definitions (e.g., `[entities.raft.model] asset = "raft"`). The archetype bundles the component schemas; the model asset is specified per-entity.

### Boarding Flow

1. Player approaches raft → `on_interact` fires (existing system)
2. Script calls `board_vessel(vessel_id)` → produces `ScriptCommand::BoardVessel { vessel_id: EntityId }`
3. `flint-player` processes command: disable player character controller, parent player to vessel at `seat_offset`, switch camera to vessel's `camera_offset`
4. Input routed to vessel: forward/back → throttle force applied to rigidbody, left/right → turn torque
5. Player presses interact again → `disembark_vessel()` → produces `ScriptCommand::DisembarkVessel`
6. Player unparented, placed beside vessel, character controller re-enabled

### New ScriptCommand Variants

```rust
// In flint-script/src/context.rs
pub enum ScriptCommand {
    // ... existing variants ...
    BoardVessel { vessel_id: EntityId },
    DisembarkVessel,
}
```

### Future: Free Movement on Deck

The seat_offset snap is the simplest boarding mode. A future "free roam on deck" mode would keep the parent relationship while re-enabling the character controller constrained to the vessel's bounds. No architectural changes needed — just a second boarding mode flag on the `vessel` component.

## 5. Swimming System (`flint-player`)

### Player Water State

The player water state is owned by `PlayerApp` in `flint-player`, NOT by `CharacterController` or `PhysicsSystem`. This keeps water logic out of the physics crate and centralized where the player state machine lives.

```rust
// In flint-player
pub enum PlayerWaterState {
    OnLand,
    Swimming,
    Underwater,
    OnVessel { vessel_id: EntityId },
}
```

Each physics tick, `PlayerApp`:
1. Queries `water_state.height_at(player_x, player_z, time)` to check player submersion
2. Updates `PlayerWaterState` based on submersion depth and input
3. Passes the water state to the character update: when `OnLand`, uses normal `CharacterController`; when `Swimming`/`Underwater`, uses the swim controller (a new movement function in `flint-player`, not in `flint-physics`)
4. Sets `is_camera_submerged` flag on the renderer for underwater post-processing

### State Transitions

```
                ┌─────────┐
                │  OnLand  │  ← character controller active
                └────┬────┘
                     │ feet submerge
                ┌────▼─────┐
                │ Swimming │  ← swim controller, locked to surface
                └────┬─────┘
                     │ dive input
               ┌─────▼──────┐
               │ Underwater │  ← 3D swim, underwater rendering
               └─────┬──────┘
                     │ ascend to surface
                ┌────▼─────┐
                │ Swimming │
                └────┬─────┘
                     │ reach shore / climb out
                ┌────▼────┐
                │  OnLand  │
                └─────────┘

On Vessel: separate state, entered via ScriptCommand, exited to Swimming or OnLand
```

### Surface Swimming

- Triggers when player's feet enter water (submersion depth check each tick)
- Replaces ground movement with swim controller (new function in `flint-player`)
- Horizontal: WASD moves along water surface plane
- Vertical: player bobs with waves (locked to surface height + offset)
- Camera stays above water at head height
- Swim speed configurable, stamina optional (script-driven)

### Diving / Underwater

- Player presses crouch/dive while swimming → submerge
- Full 3D movement (swim in any direction)
- Gravity replaced with slow sink + swim force
- Camera triggers underwater rendering mode (sets `is_camera_submerged` on renderer)
- Jump/ascend key returns to surface
- Breath/oxygen system is script-driven (not engine-level)

## 6. Script API Extensions (`flint-script`)

### Query Functions

All water queries use `f64` at the Rhai boundary (Rhai's FLOAT type), converting to/from `f32` internally. This matches existing patterns like `toml_f64`.

```rust
water_enabled() -> bool
water_height_at(x: f64, z: f64) -> f64
is_submerged(entity_id: i64) -> bool
submersion_depth(entity_id: i64) -> f64
```

Note: `water_depth_at()` (terrain-to-surface) is removed from the API. Scripts that need this can compute it themselves: `water_height_at(x, z) - terrain_height_at(x, z)`. This avoids adding a `flint-terrain` dependency to `flint-water` or `flint-script`.

### Player State

```rust
is_swimming() -> bool
is_underwater() -> bool
is_on_vessel() -> bool
current_vessel() -> i64              // entity id or -1
```

### Vessel Control

```rust
board_vessel(vessel_id: i64)         // produces ScriptCommand::BoardVessel
disembark_vessel()                   // produces ScriptCommand::DisembarkVessel
```

### New Callbacks

Following the existing callback naming pattern (`on_<event>`):

```rust
on_enter_water()    // player starts swimming
on_exit_water()     // player reaches land
on_submerge()       // player dives under
on_surface()        // player returns to surface
```

### New GameEvent Variants

```rust
// In flint-runtime/src/event_bus.rs
pub enum GameEvent {
    // ... existing variants ...
    PlayerEnteredWater,
    PlayerExitedWater,
    PlayerSubmerged,
    PlayerSurfaced,
    PlayerBoardedVessel { vessel_id: EntityId },
    PlayerDisembarkedVessel,
}
```

## 7. Audio Integration

All audio is script-driven using existing `audio_source` + callbacks — no new audio engine code:

- **Ambient ocean** — looping wave sound, volume scales with proximity to water surface
- **Splash** — on entering water, on wave crests hitting entities
- **Underwater** — low-pass filter on all audio when submerged (Kira supports this via effects chain)
- **Boat** — hull creaking, water slapping (script-triggered via existing audio system)

## 8. `flint render` — `--time` Flag

The `--time <f32>` CLI flag is added to `flint-cli`'s render subcommand. It sets the simulation time used for water wave evaluation:

1. CLI parses `--time` flag (default: `0.0` for deterministic headless renders)
2. Value passed to `HeadlessContext` as `render_time: f32`
3. Uploaded as the `time` uniform in `WaterUniforms` (same field used by the game clock in windowed mode)
4. Water shader evaluates Gerstner waves at this frozen time

This makes headless water renders fully deterministic and comparable across runs.

## 9. Showcase Scene: "Island Cove"

A small tropical island surrounded by open ocean, demonstrating every water feature.

### Elements

- Small heightmap island with sandy beach edges
- Terrain splat: sand, grass, rock layers
- Procgen trees (existing `tree_v1`)
- Rock formations near shore
- Warm skybox HDR (sunset/sunrise)
- Directional sun with volumetric god rays
- Raft with buoyancy — board and sail around island
- Swimming from beach into water
- Diving underwater near rocks

### Scene TOML

```toml
[scene]
name = "Island Cove"
description = "Water system showcase — ocean, boats, swimming"

[camera]
position = [0, 8, -15]
target = [0, 1, 0]
fov = 60.0
far = 2000.0

[environment]
skybox = "island_cove/sunset.hdr"

[water]
enabled = true
water_level = 0.0
shallow_color = [0.15, 0.5, 0.45, 0.8]
deep_color = [0.03, 0.1, 0.15, 0.95]
foam_color = [0.92, 0.96, 0.96, 0.7]
depth_fade = 10.0
foam_threshold = 0.55

[[water.waves]]
amplitude = 0.8
wavelength = 35.0
speed = 2.5
direction = [1.0, 0.2]
steepness = 0.35

[[water.waves]]
amplitude = 0.4
wavelength = 12.0
speed = 1.8
direction = [0.5, 0.9]
steepness = 0.3

[[water.waves]]
amplitude = 0.12
wavelength = 4.0
speed = 1.2
direction = [-0.4, 0.7]
steepness = 0.45

[post_process]
bloom_enabled = true
fog_enabled = true
volumetric_enabled = true

[entities.island]
archetype = "terrain"
# heightmap, splat, textures...

[entities.raft]
archetype = "vessel"
# buoyant + vessel + rigidbody + model...

[entities.player]
archetype = "player"
# character_controller + script...
```

### `flint render` Validation

```bash
# Basic ocean render
flint render demo/island_cove.scene.toml -o water_test.png \
  --distance 30 --pitch 15 --yaw 90 --time 0.0

# Shore foam — camera near beach
flint render demo/island_cove.scene.toml -o foam_test.png \
  --distance 10 --pitch 25 --target 5,0,5 --time 2.0

# Different wave states for comparison
flint render demo/island_cove.scene.toml -o wave_t0.png --time 0.0
flint render demo/island_cove.scene.toml -o wave_t5.png --time 5.0

# Underwater — requires new --camera-pos flag (added alongside --time)
flint render demo/island_cove.scene.toml -o underwater_test.png \
  --camera-pos 0,-3,0 --target 5,-2,10 --time 1.0
```

Note: `--camera-pos` is a new flag added to `flint render` alongside `--time`, providing direct camera placement as an alternative to the orbit camera (`--distance`/`--pitch`/`--yaw`). Useful for underwater shots where orbit camera cannot reach.

## 10. Future Extensions (Not in v1)

- Free movement on vessel decks (second boarding mode)
- Caustics (projected texture on underwater surfaces)
- Rivers / bounded water volumes (flow-map-driven)
- Floating debris (buoyancy on arbitrary small rigidbodies)
- Shoreline wave breaking near shallow areas
- Water procgen (`water_v1` generator for foam/normal textures)
