# Water System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ocean water to Flint — Gerstner wave simulation, stylized rendering, buoyancy physics, swimming/underwater gameplay, and a showcase scene.

**Architecture:** New `flint-water` crate (depends only on `flint-core`) owns wave math and query API. `flint-render` gets a `WaterPipeline` with projected grid shader. `flint-physics` gets buoyancy via `apply_buoyancy()`. `flint-player` owns swimming state machine and vessel boarding. `flint-scene` parses `[water]` TOML blocks.

**Tech Stack:** Rust, wgpu 23, WGSL shaders, Rapier 3D, Rhai scripting, TOML scene format

**Spec:** `docs/superpowers/specs/2026-03-29-water-system-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `crates/flint-water/Cargo.toml` | Crate manifest (depends on `flint-core`, `serde`) |
| `crates/flint-water/src/lib.rs` | Module root, re-exports |
| `crates/flint-water/src/config.rs` | `WaterConfig`, `WaveLayer` structs |
| `crates/flint-water/src/gerstner.rs` | Gerstner wave math (single wave + sum) |
| `crates/flint-water/src/state.rs` | `WaterState` query API (`height_at`, `normal_at`, etc.) |
| `crates/flint-render/src/water_pipeline.rs` | `WaterPipeline`, bind group layouts, projected grid mesh |
| `crates/flint-render/src/water_shader.wgsl` | Water vertex + fragment shader |
| `schemas/components/buoyant.toml` | Buoyancy component schema |
| `schemas/components/vessel.toml` | Vessel component schema |
| `schemas/archetypes/vessel.toml` | Vessel archetype (rigidbody + buoyant + vessel + interactable) |
| `demo/island_cove.scene.toml` | Showcase scene |

### Modified files

| File | Change |
|------|--------|
| `Cargo.toml` (workspace root) | Add `flint-water` to members, default-members, workspace deps |
| `crates/flint-scene/src/format.rs` | Add `WaterDef`, `WaveLayerDef` structs; add `water` field to `SceneFile` |
| `crates/flint-render/src/lib.rs` | Add `water_pipeline` module + exports |
| `crates/flint-render/src/scene_renderer/mod.rs` | Add water pipeline fields to `SceneRenderer`, init in `new()` |
| `crates/flint-render/src/scene_renderer/render_passes.rs` | Split main pass for depth copy, add water render call |
| `crates/flint-render/src/context.rs` | Add `COPY_SRC` to depth texture usage flags |
| `crates/flint-render/src/headless.rs` | Add `COPY_SRC` to depth texture usage flags |
| `crates/flint-render/src/postprocess.rs` | Add `is_camera_submerged` + underwater fog mode to `PostProcessConfig` and composite uniforms |
| `crates/flint-render/src/composite_shader.wgsl` | Underwater fog/tint branch in composite pass |
| `crates/flint-physics/src/lib.rs` | Add `apply_buoyancy()` method to `PhysicsSystem` |
| `crates/flint-physics/Cargo.toml` | Add `flint-water` dependency |
| `crates/flint-runtime/src/event.rs` | Add water `GameEvent` variants |
| `crates/flint-script/src/context.rs` | Add `BoardVessel`/`DisembarkVessel` to `ScriptCommand` |
| `crates/flint-script/Cargo.toml` | Add `flint-water` dependency |
| `crates/flint-player/Cargo.toml` | Add `flint-water` dependency |
| `crates/flint-player/src/player_app/mod.rs` | Add `WaterState` to `PlayerApp`, water update in game loop, swim controller, vessel boarding |
| `crates/flint-cli/src/commands/render.rs` | Add `--time` and `--camera-pos` flags to `RenderArgs` |

---

## Task 1: Create `flint-water` Crate — Config & Wave Math

**Files:**
- Create: `crates/flint-water/Cargo.toml`
- Create: `crates/flint-water/src/lib.rs`
- Create: `crates/flint-water/src/config.rs`
- Create: `crates/flint-water/src/gerstner.rs`
- Create: `crates/flint-water/src/state.rs`
- Modify: `Cargo.toml` (workspace root, lines 3-29, 32-55)

- [ ] **Step 1: Add `flint-water` to workspace**

In `Cargo.toml` (workspace root), add to `members` after line 22 (`"crates/flint-terrain"`):
```toml
    "crates/flint-water",
```
Add to `default-members` after line 51 (`"crates/flint-terrain"`):
```toml
    "crates/flint-water",
```
Add to `[workspace.dependencies]` section (find alphabetical position near other flint crates):
```toml
flint-water = { path = "crates/flint-water" }
```

- [ ] **Step 2: Create crate manifest**

Create `crates/flint-water/Cargo.toml`:
```toml
[package]
name = "flint-water"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
flint-core = { workspace = true }
serde = { workspace = true }

[dev-dependencies]
```

- [ ] **Step 3: Write failing tests for Gerstner wave math**

Create `crates/flint-water/src/gerstner.rs`:
```rust
use flint_core::Vec3;
use serde::{Serialize, Deserialize};

/// Parameters for a single Gerstner wave
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveLayer {
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub direction: [f32; 2],
    pub steepness: f32,
}

/// Evaluate a single Gerstner wave displacement at (x, z) for given time.
/// Returns the displaced (x, y, z) position.
pub fn gerstner_wave(wave: &WaveLayer, x: f32, z: f32, time: f32) -> Vec3 {
    todo!()
}

/// Evaluate the analytical normal for a single Gerstner wave at (x, z, time).
pub fn gerstner_normal(wave: &WaveLayer, x: f32, z: f32, time: f32) -> Vec3 {
    todo!()
}

/// Sum multiple Gerstner waves. Returns displaced position.
pub fn gerstner_sum(waves: &[WaveLayer], x: f32, z: f32, time: f32) -> Vec3 {
    todo!()
}

/// Sum multiple Gerstner wave normals. Returns combined normal (normalized).
pub fn gerstner_normal_sum(waves: &[WaveLayer], x: f32, z: f32, time: f32) -> Vec3 {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_wave() -> WaveLayer {
        WaveLayer {
            amplitude: 1.0,
            wavelength: 10.0,
            speed: 1.0,
            direction: [1.0, 0.0],
            steepness: 0.5,
        }
    }

    #[test]
    fn gerstner_wave_at_time_zero_origin() {
        let wave = test_wave();
        let pos = gerstner_wave(&wave, 0.0, 0.0, 0.0);
        // At t=0, x=0: phase = 0, cos(0) = 1, sin(0) = 0
        // Gerstner: x_disp = -(steepness * amplitude * dx * sin(phase)) = 0
        //           y_disp = amplitude * cos(phase) = 1.0
        assert!((pos.y - 1.0).abs() < 0.001, "y={}", pos.y);
        assert!(pos.x.abs() < 0.001, "x={}", pos.x);
    }

    #[test]
    fn gerstner_wave_produces_horizontal_displacement() {
        let wave = test_wave();
        // At phase = pi/2: sin = 1, cos = 0 → max horizontal displacement
        let k = 2.0 * std::f32::consts::PI / wave.wavelength;
        let quarter_wavelength = std::f32::consts::PI / (2.0 * k);
        let pos = gerstner_wave(&wave, quarter_wavelength, 0.0, 0.0);
        // y should be ~0 (cos(pi/2) ≈ 0)
        assert!(pos.y.abs() < 0.01, "y should be ~0, got {}", pos.y);
        // x should have negative displacement (wave steepness pulls toward crest)
        assert!(pos.x < quarter_wavelength, "x should be displaced backward");
    }

    #[test]
    fn gerstner_normal_points_up_at_trough() {
        let wave = test_wave();
        // At phase = pi (trough): normal should point mostly up
        let k = 2.0 * std::f32::consts::PI / wave.wavelength;
        let half_wavelength = std::f32::consts::PI / k;
        let normal = gerstner_normal(&wave, half_wavelength, 0.0, 0.0);
        assert!(normal.y > 0.9, "normal.y={}, expected mostly up", normal.y);
    }

    #[test]
    fn gerstner_sum_of_zero_waves_returns_flat() {
        let pos = gerstner_sum(&[], 5.0, 3.0, 1.0);
        assert!((pos.x - 5.0).abs() < 0.001);
        assert!(pos.y.abs() < 0.001);
        assert!((pos.z - 3.0).abs() < 0.001);
    }

    #[test]
    fn gerstner_sum_multiple_waves_combines() {
        let waves = vec![test_wave(), test_wave()];
        let single = gerstner_wave(&test_wave(), 0.0, 0.0, 0.0);
        let summed = gerstner_sum(&waves, 0.0, 0.0, 0.0);
        // Two identical waves at same point = 2x displacement
        assert!((summed.y - 2.0 * single.y).abs() < 0.01);
    }

    #[test]
    fn gerstner_normal_sum_empty_returns_up() {
        let n = gerstner_normal_sum(&[], 0.0, 0.0, 0.0);
        assert!((n.y - 1.0).abs() < 0.001);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p flint-water`
Expected: FAIL — `todo!()` panics

- [ ] **Step 5: Implement Gerstner wave math**

Replace `todo!()` in `gerstner.rs` with the actual Gerstner formulas:

```rust
pub fn gerstner_wave(wave: &WaveLayer, x: f32, z: f32, time: f32) -> Vec3 {
    let k = 2.0 * std::f32::consts::PI / wave.wavelength;
    let omega = k * wave.speed;
    let dir_len = (wave.direction[0] * wave.direction[0] + wave.direction[1] * wave.direction[1]).sqrt();
    if dir_len < 1e-6 || wave.wavelength < 1e-6 {
        return Vec3 { x, y: 0.0, z };
    }
    let dx = wave.direction[0] / dir_len;
    let dz = wave.direction[1] / dir_len;
    let phase = k * (dx * x + dz * z) - omega * time;
    let q = wave.steepness / (k * wave.amplitude).max(1e-6); // Gerstner Q parameter

    Vec3 {
        x: x - q * wave.amplitude * dx * phase.sin(),
        y: wave.amplitude * phase.cos(),
        z: z - q * wave.amplitude * dz * phase.sin(),
    }
}

pub fn gerstner_normal(wave: &WaveLayer, x: f32, z: f32, time: f32) -> Vec3 {
    let k = 2.0 * std::f32::consts::PI / wave.wavelength;
    let omega = k * wave.speed;
    let dir_len = (wave.direction[0] * wave.direction[0] + wave.direction[1] * wave.direction[1]).sqrt();
    if dir_len < 1e-6 || wave.wavelength < 1e-6 {
        return Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    }
    let dx = wave.direction[0] / dir_len;
    let dz = wave.direction[1] / dir_len;
    let phase = k * (dx * x + dz * z) - omega * time;
    let q = wave.steepness / (k * wave.amplitude).max(1e-6);

    // Analytical Gerstner normal components
    let nx = -dx * wave.amplitude * k * phase.cos();
    let nz = -dz * wave.amplitude * k * phase.cos();
    let ny = 1.0 - q * wave.amplitude * k * phase.sin();

    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    Vec3 { x: nx / len, y: ny / len, z: nz / len }
}

pub fn gerstner_sum(waves: &[WaveLayer], x: f32, z: f32, time: f32) -> Vec3 {
    let mut result = Vec3 { x, y: 0.0, z };
    for wave in waves {
        let displaced = gerstner_wave(wave, x, z, time);
        result.x += displaced.x - x; // accumulate displacement only
        result.y += displaced.y;
        result.z += displaced.z - z;
    }
    result
}

pub fn gerstner_normal_sum(waves: &[WaveLayer], x: f32, z: f32, time: f32) -> Vec3 {
    if waves.is_empty() {
        return Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    }
    let mut nx = 0.0f32;
    let mut nz = 0.0f32;
    let mut ny = 1.0f32; // start with up, subtract perturbations
    for wave in waves {
        let n = gerstner_normal(wave, x, z, time);
        nx += n.x;
        nz += n.z;
        ny += n.y - 1.0; // accumulate deviation from up
    }
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    Vec3 { x: nx / len, y: ny / len, z: nz / len }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p flint-water`
Expected: All 6 tests PASS

- [ ] **Step 7: Write `WaterConfig` and `WaterState`**

Create `crates/flint-water/src/config.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Runtime water configuration (constructed from scene WaterDef)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub waves: Vec<crate::gerstner::WaveLayer>,
}

impl Default for WaterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            water_level: 0.0,
            inverse_solve_iterations: 3,
            shallow_color: [0.1, 0.4, 0.35, 0.85],
            deep_color: [0.02, 0.08, 0.12, 0.95],
            foam_color: [0.9, 0.95, 0.95, 0.8],
            depth_fade: 8.0,
            foam_threshold: 0.6,
            fresnel_power: 5.0,
            normal_map: None,
            foam_texture: None,
            waves: vec![],
        }
    }
}
```

Create `crates/flint-water/src/state.rs` with tests:
```rust
use flint_core::Vec3;
use crate::config::WaterConfig;
use crate::gerstner;

pub struct WaterState {
    config: WaterConfig,
}

impl WaterState {
    pub fn new(config: WaterConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &WaterConfig {
        &self.config
    }

    pub fn surface_point(&self, x: f32, z: f32, time: f32) -> Vec3 {
        let mut result = gerstner::gerstner_sum(&self.config.waves, x, z, time);
        result.y += self.config.water_level;
        result
    }

    pub fn height_at(&self, x: f32, z: f32, time: f32) -> f32 {
        // Iterative inverse solve: Gerstner displaces horizontally,
        // so we need to find which undisplaced (x', z') maps to our query (x, z)
        let mut guess_x = x;
        let mut guess_z = z;
        for _ in 0..self.config.inverse_solve_iterations {
            let displaced = gerstner::gerstner_sum(&self.config.waves, guess_x, guess_z, time);
            guess_x += x - displaced.x;
            guess_z += z - displaced.z;
        }
        let final_pos = gerstner::gerstner_sum(&self.config.waves, guess_x, guess_z, time);
        final_pos.y + self.config.water_level
    }

    pub fn normal_at(&self, x: f32, z: f32, time: f32) -> Vec3 {
        gerstner::gerstner_normal_sum(&self.config.waves, x, z, time)
    }

    pub fn velocity_at(&self, x: f32, z: f32, time: f32) -> Vec3 {
        // Finite difference approximation of wave velocity
        let dt = 0.01;
        let p0 = self.surface_point(x, z, time);
        let p1 = self.surface_point(x, z, time + dt);
        Vec3 {
            x: (p1.x - p0.x) / dt,
            y: (p1.y - p0.y) / dt,
            z: (p1.z - p0.z) / dt,
        }
    }

    pub fn foam_at(&self, x: f32, z: f32, time: f32) -> f32 {
        // Foam based on vertical displacement relative to amplitude sum
        if self.config.waves.is_empty() {
            return 0.0;
        }
        let max_amp: f32 = self.config.waves.iter().map(|w| w.amplitude).sum();
        if max_amp < 1e-6 {
            return 0.0;
        }
        let height = self.height_at(x, z, time) - self.config.water_level;
        let normalized = height / max_amp;
        ((normalized - self.config.foam_threshold) / (1.0 - self.config.foam_threshold))
            .clamp(0.0, 1.0)
    }

    pub fn is_submerged(&self, pos: Vec3, time: f32) -> bool {
        pos.y < self.height_at(pos.x, pos.z, time)
    }

    pub fn submersion_depth(&self, pos: Vec3, time: f32) -> f32 {
        self.height_at(pos.x, pos.z, time) - pos.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerstner::WaveLayer;

    fn calm_water() -> WaterState {
        WaterState::new(WaterConfig {
            waves: vec![],
            ..WaterConfig::default()
        })
    }

    fn wavy_water() -> WaterState {
        WaterState::new(WaterConfig {
            waves: vec![WaveLayer {
                amplitude: 1.0,
                wavelength: 10.0,
                speed: 1.0,
                direction: [1.0, 0.0],
                steepness: 0.3,
            }],
            ..WaterConfig::default()
        })
    }

    #[test]
    fn calm_water_height_is_water_level() {
        let water = calm_water();
        let h = water.height_at(5.0, 3.0, 0.0);
        assert!((h - 0.0).abs() < 0.001);
    }

    #[test]
    fn calm_water_height_respects_water_level() {
        let mut config = WaterConfig::default();
        config.water_level = 5.0;
        let water = WaterState::new(config);
        let h = water.height_at(0.0, 0.0, 0.0);
        assert!((h - 5.0).abs() < 0.001);
    }

    #[test]
    fn wavy_water_height_varies() {
        let water = wavy_water();
        let h1 = water.height_at(0.0, 0.0, 0.0);
        let h2 = water.height_at(5.0, 0.0, 0.0);
        assert!((h1 - h2).abs() > 0.01, "heights should differ: {} vs {}", h1, h2);
    }

    #[test]
    fn is_submerged_below_surface() {
        let water = calm_water();
        assert!(water.is_submerged(Vec3 { x: 0.0, y: -1.0, z: 0.0 }, 0.0));
        assert!(!water.is_submerged(Vec3 { x: 0.0, y: 1.0, z: 0.0 }, 0.0));
    }

    #[test]
    fn submersion_depth_correct() {
        let water = calm_water();
        let d = water.submersion_depth(Vec3 { x: 0.0, y: -3.0, z: 0.0 }, 0.0);
        assert!((d - 3.0).abs() < 0.001);
    }

    #[test]
    fn foam_zero_for_calm_water() {
        let water = calm_water();
        assert!((water.foam_at(0.0, 0.0, 0.0)).abs() < 0.001);
    }

    #[test]
    fn normal_points_up_for_calm_water() {
        let water = calm_water();
        let n = water.normal_at(0.0, 0.0, 0.0);
        assert!((n.y - 1.0).abs() < 0.001);
    }
}
```

Create `crates/flint-water/src/lib.rs`:
```rust
pub mod config;
pub mod gerstner;
pub mod state;

pub use config::{WaterConfig};
pub use gerstner::WaveLayer;
pub use state::WaterState;
```

- [ ] **Step 8: Run all tests**

Run: `cargo test -p flint-water`
Expected: All tests PASS (gerstner + state)

- [ ] **Step 9: Verify workspace compiles**

Run: `cargo build -p flint-water`
Expected: Clean build

- [ ] **Step 10: Commit**

```bash
git add crates/flint-water/ Cargo.toml
git commit -m "feat(water): add flint-water crate with Gerstner wave simulation and query API"
```

---

## Task 2: Scene Format — `WaterDef` and TOML Parsing

**Files:**
- Modify: `crates/flint-scene/src/format.rs` (lines 7-20 for SceneFile, after line 198 for defaults)

- [ ] **Step 1: Write failing test for WaterDef parsing**

Add test to `crates/flint-scene/src/format.rs` (or the crate's test module). The test should parse a TOML string with a `[water]` block and verify the fields deserialize correctly:

```rust
#[test]
fn parse_water_def() {
    let toml_str = r#"
[scene]
name = "test"
version = "1.0"

[water]
enabled = true
water_level = 2.5
shallow_color = [0.1, 0.4, 0.35, 0.85]
foam_threshold = 0.7

[[water.waves]]
amplitude = 1.2
wavelength = 40.0
speed = 3.0
direction = [1.0, 0.3]
steepness = 0.4
"#;
    let scene: SceneFile = toml::from_str(toml_str).unwrap();
    let water = scene.water.unwrap();
    assert_eq!(water.water_level, 2.5);
    assert_eq!(water.foam_threshold, 0.7);
    assert_eq!(water.waves.len(), 1);
    assert!((water.waves[0].amplitude - 1.2).abs() < 0.001);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p flint-scene parse_water_def`
Expected: FAIL — `water` field doesn't exist on `SceneFile`

- [ ] **Step 3: Add `WaterDef`, `WaveLayerDef` to `format.rs`**

In `crates/flint-scene/src/format.rs`, add after `PostProcessDef` (after line 110):

```rust
/// Water configuration for the scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterDef {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub water_level: f32,
    #[serde(default = "default_inverse_solve_iterations")]
    pub inverse_solve_iterations: u32,
    #[serde(default = "default_shallow_color")]
    pub shallow_color: [f32; 4],
    #[serde(default = "default_deep_color")]
    pub deep_color: [f32; 4],
    #[serde(default = "default_foam_color")]
    pub foam_color: [f32; 4],
    #[serde(default = "default_depth_fade")]
    pub depth_fade: f32,
    #[serde(default = "default_foam_threshold")]
    pub foam_threshold: f32,
    #[serde(default = "default_fresnel_power")]
    pub fresnel_power: f32,
    #[serde(default)]
    pub normal_map: Option<String>,
    #[serde(default)]
    pub foam_texture: Option<String>,
    #[serde(default)]
    pub waves: Vec<WaveLayerDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveLayerDef {
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub direction: [f32; 2],
    pub steepness: f32,
}
```

Add the `water` field to `SceneFile` (line 15, after `post_process`):
```rust
    #[serde(default)]
    pub water: Option<WaterDef>,
```

Add default functions after the existing ones (after line 198):
```rust
fn default_inverse_solve_iterations() -> u32 { 3 }
fn default_shallow_color() -> [f32; 4] { [0.1, 0.4, 0.35, 0.85] }
fn default_deep_color() -> [f32; 4] { [0.02, 0.08, 0.12, 0.95] }
fn default_foam_color() -> [f32; 4] { [0.9, 0.95, 0.95, 0.8] }
fn default_depth_fade() -> f32 { 8.0 }
fn default_foam_threshold() -> f32 { 0.6 }
fn default_fresnel_power() -> f32 { 5.0 }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p flint-scene parse_water_def`
Expected: PASS

- [ ] **Step 5: Add test for WaterDef defaults and multi-wave parsing**

```rust
#[test]
fn parse_water_def_defaults() {
    let toml_str = r#"
[scene]
name = "test"
version = "1.0"

[water]
"#;
    let scene: SceneFile = toml::from_str(toml_str).unwrap();
    let water = scene.water.unwrap();
    assert_eq!(water.enabled, true);
    assert!((water.water_level - 0.0).abs() < 0.001);
    assert_eq!(water.inverse_solve_iterations, 3);
    assert!((water.depth_fade - 8.0).abs() < 0.001);
    assert!(water.waves.is_empty());
}

#[test]
fn parse_water_def_multiple_waves() {
    let toml_str = r#"
[scene]
name = "test"
version = "1.0"

[water]
water_level = 1.0

[[water.waves]]
amplitude = 1.0
wavelength = 10.0
speed = 2.0
direction = [1.0, 0.0]
steepness = 0.3

[[water.waves]]
amplitude = 0.5
wavelength = 5.0
speed = 1.5
direction = [0.0, 1.0]
steepness = 0.4

[[water.waves]]
amplitude = 0.2
wavelength = 3.0
speed = 1.0
direction = [0.5, 0.5]
steepness = 0.5
"#;
    let scene: SceneFile = toml::from_str(toml_str).unwrap();
    let water = scene.water.unwrap();
    assert_eq!(water.waves.len(), 3);
    assert!((water.waves[2].steepness - 0.5).abs() < 0.001);
}
```

Run: `cargo test -p flint-scene parse_water`
Expected: All 3 water tests PASS

- [ ] **Step 6: Verify full crate compiles and existing tests pass**

Run: `cargo test -p flint-scene`
Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/flint-scene/src/format.rs
git commit -m "feat(scene): add WaterDef and WaveLayerDef to scene format"
```

---

## Task 3: Component Schemas — `buoyant`, `vessel`, Archetype

**Files:**
- Create: `schemas/components/buoyant.toml`
- Create: `schemas/components/vessel.toml`
- Create: `schemas/archetypes/vessel.toml`

- [ ] **Step 1: Create `buoyant` component schema**

Create `schemas/components/buoyant.toml`:
```toml
[component.buoyant]
description = "Makes a rigidbody float on water surfaces"
buoyancy_force = { type = "float", default = 10.0, description = "Upward force multiplier per unit submersion" }
drag = { type = "float", default = 1.5, description = "Linear drag in water" }
angular_drag = { type = "float", default = 2.0, description = "Angular drag in water (prevents endless rocking)" }
```

- [ ] **Step 2: Create `vessel` component schema**

Create `schemas/components/vessel.toml`:
```toml
[component.vessel]
description = "Rideable watercraft — player can board and steer"
seat_offset = { type = "vec3", default = [0, 0.5, 0], description = "Local-space offset where player sits" }
throttle_force = { type = "float", default = 15.0, description = "Forward force when accelerating" }
turn_torque = { type = "float", default = 8.0, description = "Torque applied when steering" }
max_speed = { type = "float", default = 12.0, description = "Maximum speed in world units/second" }
camera_offset = { type = "vec3", default = [0, 3, -8], description = "Camera offset from vessel when boarded" }
```

- [ ] **Step 3: Create `vessel` archetype**

Create `schemas/archetypes/vessel.toml`:
```toml
[archetype.vessel]
description = "Rideable watercraft with buoyancy"
components = ["transform", "rigidbody", "collider", "buoyant", "vessel", "interactable"]

[archetype.vessel.defaults.buoyant]
buoyancy_force = 12.0
drag = 1.5

[archetype.vessel.defaults.interactable]
prompt_text = "Board"
range = 3.0

[archetype.vessel.defaults.rigidbody]
mass = 50.0

[archetype.vessel.defaults.collider]
shape = "box"
size = [4, 0.6, 6]
```

- [ ] **Step 4: Validate schemas load**

Run: `cargo run --bin flint -- schema list --schemas schemas`
Expected: `buoyant` and `vessel` appear in output; no parse errors

- [ ] **Step 5: Commit**

```bash
git add schemas/components/buoyant.toml schemas/components/vessel.toml schemas/archetypes/vessel.toml
git commit -m "feat(schemas): add buoyant component, vessel component, and vessel archetype"
```

---

## Task 4: Water Rendering Pipeline — `WaterPipeline` + Shader

**Files:**
- Create: `crates/flint-render/src/water_pipeline.rs`
- Create: `crates/flint-render/src/water_shader.wgsl`
- Modify: `crates/flint-render/src/lib.rs` (lines 8-31 modules, lines 33-67 exports)
- Modify: `crates/flint-render/src/scene_renderer/mod.rs` (lines 110-178 struct, lines 194-273 init)

- [ ] **Step 1: Create the water WGSL shader**

Create `crates/flint-render/src/water_shader.wgsl`. This is a large file — the full Gerstner vertex shader + stylized fragment shader. Key sections:

- Uniform structs: `WaterUniforms` (view_proj, inv_view_proj, camera_pos, time, water_level, grid_size, grid_extent, wave_count, array of 8 WaveLayers)
- `WaterMaterialUniforms` (shallow_color, deep_color, foam_color, depth_fade, foam_threshold, fresnel_power)
- Vertex shader: project grid quad onto water plane via inv_view_proj, displace with Gerstner sum, compute foam factor
- Fragment shader: Fresnel (Schlick), depth-based color blend (sample scene_depth), scrolling detail normals (2 layers), foam, Blinn-Phong sun specular

The shader should be ~250-350 lines of WGSL. Write the full shader with all bind group bindings matching the pipeline layout spec.

- [ ] **Step 2: Create `WaterPipeline` Rust struct**

Create `crates/flint-render/src/water_pipeline.rs`:

- `WaterPipeline` struct: holds `wgpu::RenderPipeline`, bind group layouts, projected grid vertex/index buffers
- `WaterUniforms` (bytemuck): matches shader struct layout exactly
- `WaterMaterialUniforms` (bytemuck): matches shader material struct
- `WaveLayerGpu` (bytemuck): 32-byte GPU representation of a wave layer
- `WaterDrawCall` struct: uniform buffer, material bind group, transform bind group
- `WaterPipeline::new()`: creates pipeline with alpha blending, depth write enabled, the 3 bind group layouts
- `generate_projected_grid()`: creates the 128×128 grid vertex/index buffers (positions are 0..1 UV space, projected in shader)

The pipeline format must match the HDR format (`wgpu::TextureFormat::Rgba16Float`) when post-processing is active, or the surface format otherwise — check how `RenderPipeline::new()` handles this in `pipeline.rs`.

- [ ] **Step 3: Export the module**

In `crates/flint-render/src/lib.rs`, add after line 30 (`pub mod terrain_pipeline;`):
```rust
pub mod water_pipeline;
```

Add to exports after line 66 (`pub use terrain_pipeline::{...};`):
```rust
pub use water_pipeline::{WaterDrawCall, WaterPipeline, WaterUniforms, WaterMaterialUniforms};
```

- [ ] **Step 4: Add water fields to `SceneRenderer`**

In `crates/flint-render/src/scene_renderer/mod.rs`, add fields to the `SceneRenderer` struct after the grass section (~line 166, before `// Particles`):

```rust
    // Water
    water_pipeline: Option<WaterPipeline>,
    water_uniform_buffer: Option<wgpu::Buffer>,
    water_uniform_bind_group: Option<wgpu::BindGroup>,
    water_material_buffer: Option<wgpu::Buffer>,
    water_material_bind_group: Option<wgpu::BindGroup>,
    water_depth_copy_texture: Option<wgpu::Texture>,
    water_depth_copy_view: Option<wgpu::TextureView>,
    water_enabled: bool,
```

Initialize these fields in `SceneRenderer::new()` (after grass pipeline init, ~line 254):
```rust
let water_pipeline = WaterPipeline::new(
    &context.device,
    scene_format,
    &pipeline.light_bind_group_layout,
);
```

Set default values (`water_enabled: false`, Option fields to `None`).

- [ ] **Step 5: Add naga parse test for the water shader**

In `crates/flint-render/src/lib.rs`, in the `#[cfg(test)] mod tests` block (after the existing shader parse tests), add:
```rust
    #[test]
    fn water_shader_wgsl_parses() {
        let source = include_str!("water_shader.wgsl");
        naga::front::wgsl::parse_str(source).expect("water_shader.wgsl failed to parse");
    }
```

Run: `cargo test -p flint-render water_shader_wgsl_parses`
Expected: PASS

- [ ] **Step 6: Verify the crate compiles**

Run: `cargo build -p flint-render`
Expected: Clean build (water pipeline exists but isn't rendered yet)

- [ ] **Step 7: Commit**

```bash
git add crates/flint-render/src/water_pipeline.rs crates/flint-render/src/water_shader.wgsl crates/flint-render/src/lib.rs crates/flint-render/src/scene_renderer/mod.rs
git commit -m "feat(render): add WaterPipeline with projected grid and Gerstner shader"
```

---

## Task 5: Render Pass Integration — Depth Copy + Water Draw

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs` (lines 518-604 main pass, lines 709+ normal pass)
- Modify: `crates/flint-render/src/scene_renderer/mod.rs` (extract method, uniform upload)

- [ ] **Step 1: Add `COPY_SRC` to depth texture usage flags**

The depth buffer copy requires `COPY_SRC` on the source texture. Modify depth texture creation:

- In `crates/flint-render/src/context.rs` (~line 184): add `| wgpu::TextureUsages::COPY_SRC` to the depth texture usage flags
- In `crates/flint-render/src/headless.rs` (~line 78): add `| wgpu::TextureUsages::COPY_SRC` to the depth texture usage flags

Both currently use `RENDER_ATTACHMENT | TEXTURE_BINDING`; add `COPY_SRC` to enable `copy_texture_to_texture`.

- [ ] **Step 2: Split the main render pass for depth copy**

In `render_passes.rs`, the `render_main_pass` function (line 518) currently runs one `begin_render_pass` for the entire main pass. Refactor it to:

1. First sub-pass: skybox, grid, terrain, grass, outlines, opaque entities, skinned entities, billboards, 2D sprites — end render pass (keeps existing draw order for all opaque/sprite content)
2. Depth buffer copy: `encoder.copy_texture_to_texture()` from depth attachment to `water_depth_copy_texture`
3. Second sub-pass: water, transparent entities, particles — begin new render pass with `LoadOp::Load` for both color and depth

The key change is splitting `render_normal_pass` into `render_opaques_pass` and `render_water_and_transparents_pass`, or inserting a pass break point. Both sub-passes use the same color and depth attachments. Important: 2D sprites stay in the opaques pass (they are drawn after billboards in the current code) to preserve existing draw order.

- [ ] **Step 3: Add the water draw call to the second sub-pass**

After the depth copy and before transparent entities, add:

```rust
// Water rendering
if self.water_enabled {
    if let (Some(wp), Some(uniform_bg), Some(mat_bg)) = (
        &self.water_pipeline,
        &self.water_uniform_bind_group,
        &self.water_material_bind_group,
    ) {
        render_pass.set_pipeline(&wp.pipeline);
        render_pass.set_bind_group(0, uniform_bg, &[]);
        render_pass.set_bind_group(1, mat_bg, &[]);
        render_pass.set_bind_group(2, &self.light_bind_group, &[]);
        render_pass.set_vertex_buffer(0, wp.grid_vertex_buffer.slice(..));
        render_pass.set_index_buffer(wp.grid_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..wp.grid_index_count, 0, 0..1);
    }
}
```

- [ ] **Step 4: Add water uniform upload method**

In `scene_renderer/mod.rs`, add a public method to upload water config:

```rust
pub fn set_water_config(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, config: &WaterConfig, time: f32, camera: &Camera) {
    // Upload WaterUniforms (view_proj, inv_view_proj, camera_pos, time, wave data)
    // Upload WaterMaterialUniforms (colors, thresholds)
    // Create/update bind groups
    // Create depth copy texture if needed (matching depth texture dimensions)
    // On resize: check if depth_copy texture dimensions match, recreate if not
    self.water_enabled = config.enabled;
}
```

Note: The depth copy texture must be recreated when the window resizes. Check dimensions against the current depth texture each time this method is called; only recreate if they differ. This avoids needing a separate resize hook.

- [ ] **Step 5: Build and verify**

Run: `cargo build -p flint-render`
Expected: Clean build. Water won't render yet (no scene loads water config), but the pipeline is wired.

- [ ] **Step 6: Commit**

```bash
git add crates/flint-render/src/scene_renderer/
git commit -m "feat(render): integrate water into render pass with depth copy split"
```

---

## Task 6: Wire Water into `flint-player` and `flint render`

**Files:**
- Modify: `crates/flint-player/Cargo.toml` (add flint-water dep)
- Modify: `crates/flint-player/src/player_app/mod.rs` (add WaterState, load from scene, pass to renderer)
- Modify: `crates/flint-cli/src/commands/render.rs` (add --time flag, pass water config to renderer)

- [ ] **Step 1: Add `flint-water` dependency to `flint-player`**

In `crates/flint-player/Cargo.toml`, add:
```toml
flint-water = { workspace = true }
```

- [ ] **Step 2: Add `WaterState` to `PlayerApp`**

In `crates/flint-player/src/player_app/mod.rs`:
- Add `water_state: Option<flint_water::WaterState>` field to `PlayerApp` struct
- In scene load / `load_scene()`: if `scene_file.water.is_some()`, construct `WaterConfig` from `WaterDef` and create `WaterState`
- In the render update (where `SceneRenderer::set_postprocess_config` is called), also call `scene_renderer.set_water_config(...)` passing the water state's config and the current game clock time

- [ ] **Step 3: Add `--time` flag to `flint render`**

In `crates/flint-cli/src/commands/render.rs`, add to `RenderArgs` (after line 47):
```rust
    pub time: Option<f32>,
    pub camera_pos: Option<[f32; 3]>,
```

In the `run()` function, after scene loading and camera setup: if the scene has a `[water]` block, construct `WaterConfig`, create `WaterState`, call `scene_renderer.set_water_config()` with `args.time.unwrap_or(0.0)`.

If `camera_pos` is provided, set the camera position directly instead of using orbit parameters.

- [ ] **Step 4: Add `--time` and `--camera-pos` to CLI argument parsing**

In the clap argument definitions (find where `RenderArgs` is constructed from CLI args, likely in `crates/flint-cli/src/main.rs` or a subcommand module), add:
```rust
#[arg(long, help = "Simulation time for water waves (default: 0.0)")]
time: Option<f32>,
#[arg(long, help = "Direct camera position x,y,z (alternative to orbit camera)")]
camera_pos: Option<String>,  // parsed as "x,y,z"
```

- [ ] **Step 5: Test with a minimal water scene**

Create a minimal test scene `demo/water_test.scene.toml`:
```toml
[scene]
name = "Water Test"
version = "1.0"

[camera]
position = [0, 10, -20]
target = [0, 0, 0]
fov = 60.0
far = 2000.0

[water]
enabled = true
water_level = 0.0
shallow_color = [0.15, 0.5, 0.45, 0.8]
deep_color = [0.03, 0.1, 0.15, 0.95]

[[water.waves]]
amplitude = 0.8
wavelength = 35.0
speed = 2.5
direction = [1.0, 0.2]
steepness = 0.35

[[water.waves]]
amplitude = 0.3
wavelength = 12.0
speed = 1.8
direction = [0.5, 0.9]
steepness = 0.3
```

Run: `cargo run --bin flint -- render demo/water_test.scene.toml -o water_test.png --schemas schemas --width 1280 --height 720 --time 0.0`
Expected: PNG output with visible ocean surface (waves, colors). This is the first visual validation.

- [ ] **Step 6: Verify deterministic rendering**

Run twice with same `--time`:
```bash
cargo run --bin flint -- render demo/water_test.scene.toml -o water_a.png --schemas schemas --time 2.5
cargo run --bin flint -- render demo/water_test.scene.toml -o water_b.png --schemas schemas --time 2.5
```
Expected: Identical output (compare file hashes or visually)

- [ ] **Step 7: Commit**

```bash
git add crates/flint-player/ crates/flint-cli/ demo/water_test.scene.toml
git commit -m "feat(player,cli): wire water rendering into player and headless render with --time flag"
```

---

## Task 7: Underwater Rendering — Post-Process Integration

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs` (PostProcessConfig, PostProcessUniforms)
- Modify: `crates/flint-render/src/composite_shader.wgsl`
- Modify: `crates/flint-render/src/scene_renderer/mod.rs` (set underwater flag)

- [ ] **Step 1: Add underwater fields to `PostProcessConfig`**

In `crates/flint-render/src/postprocess.rs`, add to `PostProcessConfig`:
```rust
    pub is_camera_submerged: bool,
    pub underwater_fog_color: [f32; 3],
    pub underwater_fog_density: f32,
    pub underwater_tint: [f32; 3],
    pub underwater_time: f32, // for wavy distortion
```

Default `is_camera_submerged` to `false`, other underwater fields to sensible defaults.

- [ ] **Step 2: Pass underwater uniforms to composite shader**

In the `PostProcessUniforms` struct (bytemuck repr), add the underwater fields so they're available in the shader. Update the uniform buffer upload to include these values.

- [ ] **Step 3: Add underwater branch to composite shader**

In `crates/flint-render/src/composite_shader.wgsl`, add after the normal fog calculation:

```wgsl
// Underwater mode: override fog with underwater parameters
if (uniforms.is_camera_submerged > 0.5) {
    // Replace atmospheric fog with underwater fog
    let underwater_fog = 1.0 - exp(-uniforms.underwater_fog_density * depth);
    color = mix(color, vec3f(uniforms.underwater_fog_color), underwater_fog);
    // Apply tint
    color *= uniforms.underwater_tint;
    // Optional wavy distortion via UV offset
    // (reuse chromatic aberration path with sinusoidal offset)
}
```

- [ ] **Step 4: Wire up in `SceneRenderer`**

When setting water config, if the camera Y is below `water.height_at(camera.x, camera.z, time)`, set `postprocess_config.is_camera_submerged = true` and populate underwater fog color/density from water config.

- [ ] **Step 5: Test underwater rendering**

Run: `cargo run --bin flint -- render demo/water_test.scene.toml -o underwater_test.png --schemas schemas --camera-pos 0,-3,0 --target 5,-2,10 --time 1.0`
Expected: Blue-tinted, foggy underwater view

- [ ] **Step 6: Commit**

```bash
git add crates/flint-render/src/postprocess.rs crates/flint-render/src/composite_shader.wgsl crates/flint-render/src/scene_renderer/
git commit -m "feat(render): add underwater fog and tint to post-processing pipeline"
```

---

## Task 8: Buoyancy Physics

**Files:**
- Modify: `crates/flint-physics/Cargo.toml` (add flint-water dep)
- Modify: `crates/flint-physics/src/lib.rs` (add `apply_buoyancy` method)

- [ ] **Step 1: Add `flint-water` dependency**

In `crates/flint-physics/Cargo.toml`, add:
```toml
flint-water = { workspace = true }
```

- [ ] **Step 2: Write failing test for buoyancy force calculation**

In `crates/flint-physics/src/lib.rs` (or a new `buoyancy.rs` module), add a unit test:

```rust
#[cfg(test)]
mod buoyancy_tests {
    use flint_water::{WaterConfig, WaterState};

    #[test]
    fn buoyancy_force_proportional_to_submersion() {
        let water = WaterState::new(WaterConfig::default());
        // A point 2.0 units below water level with buoyancy_force=10.0
        // should produce force = 10.0 * 2.0 = 20.0
        let force = compute_point_buoyancy(0.0, -2.0, 0.0, 10.0, &water, 0.0);
        assert!((force - 20.0).abs() < 0.1);
    }

    #[test]
    fn no_buoyancy_above_water() {
        let water = WaterState::new(WaterConfig::default());
        let force = compute_point_buoyancy(0.0, 5.0, 0.0, 10.0, &water, 0.0);
        assert!((force).abs() < 0.001);
    }
}
```

- [ ] **Step 3: Implement buoyancy**

Add `apply_buoyancy` method to `PhysicsSystem`:

```rust
pub fn apply_buoyancy(
    &mut self,
    world: &mut FlintWorld,
    water: &flint_water::WaterState,
    time: f32,
) {
    // For each entity with "buoyant" component:
    // 1. Read buoyancy_force, drag, angular_drag from component data
    // 2. Get entity transform (position) and bounds (for sample points)
    // 3. Generate sample points from bounds (corners + center) or use defaults
    // 4. For each sample point:
    //    a. Transform to world space
    //    b. submersion = water.height_at(x, z, time) - point.y
    //    c. If submersion > 0: apply upward force = buoyancy_force * submersion
    // 5. Apply linear drag and angular drag to rigidbody
    // 6. Apply current force from water.velocity_at()
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p flint-physics buoyancy`
Expected: PASS

- [ ] **Step 5: Wire into player game loop**

In `crates/flint-player/src/player_app/mod.rs`, in the fixed update loop (after `self.physics.fixed_update()`), add:

```rust
if let Some(ref water) = self.water_state {
    if water.config().enabled {
        let time = self.clock.elapsed_secs() as f32;
        self.physics.apply_buoyancy(&mut self.world, water, time);
    }
}
```

- [ ] **Step 6: Build and verify**

Run: `cargo build -p flint-player`
Expected: Clean build

- [ ] **Step 7: Commit**

```bash
git add crates/flint-physics/ crates/flint-player/
git commit -m "feat(physics): add buoyancy force calculation using water query API"
```

---

## Task 9: GameEvents + ScriptCommands for Water

**Files:**
- Modify: `crates/flint-runtime/src/event.rs` (lines 6-28)
- Modify: `crates/flint-script/src/context.rs` (lines 79-130)
- Modify: `crates/flint-script/Cargo.toml` (add flint-water dep)

- [ ] **Step 1: Add water GameEvent variants**

In `crates/flint-runtime/src/event.rs`, add to the `GameEvent` enum (after line 27):

```rust
    /// Player entered water (started swimming)
    PlayerEnteredWater,
    /// Player exited water (reached land)
    PlayerExitedWater,
    /// Player submerged underwater
    PlayerSubmerged,
    /// Player returned to surface
    PlayerSurfaced,
    /// Player boarded a vessel
    PlayerBoardedVessel { vessel_id: EntityId },
    /// Player disembarked a vessel
    PlayerDisembarkedVessel,
```

- [ ] **Step 2: Add vessel ScriptCommands**

In `crates/flint-script/src/context.rs`, add to `ScriptCommand` enum (after line 129):

```rust
    BoardVessel { vessel_id: i64 },
    DisembarkVessel,
```

- [ ] **Step 3: Add water script query functions**

Add `flint-water` dependency to `crates/flint-script/Cargo.toml`:
```toml
flint-water = { workspace = true }
```

In the script registration code (find where Rhai functions are registered), add:
```rust
// Water queries
engine.register_fn("water_enabled", |ctx: &mut ScriptCallContext| -> bool { ... });
engine.register_fn("water_height_at", |ctx: &mut ScriptCallContext, x: f64, z: f64| -> f64 { ... });
engine.register_fn("is_submerged", |ctx: &mut ScriptCallContext, entity_id: i64| -> bool { ... });
engine.register_fn("submersion_depth", |ctx: &mut ScriptCallContext, entity_id: i64| -> f64 { ... });
engine.register_fn("is_swimming", |ctx: &mut ScriptCallContext| -> bool { ... });
engine.register_fn("is_underwater", |ctx: &mut ScriptCallContext| -> bool { ... });
engine.register_fn("is_on_vessel", |ctx: &mut ScriptCallContext| -> bool { ... });
engine.register_fn("current_vessel", |ctx: &mut ScriptCallContext| -> i64 { ... });
engine.register_fn("board_vessel", |ctx: &mut ScriptCallContext, vessel_id: i64| { ... });
engine.register_fn("disembark_vessel", |ctx: &mut ScriptCallContext| { ... });
```

The water query functions need access to `WaterState`. Follow the existing `terrain_height_fn` pattern in `context.rs` (~line 273): store callback closures like `water_height_fn: Option<Box<dyn Fn(f32, f32) -> f32 + Send + Sync>>` and `water_submerged_fn: Option<Box<dyn Fn(f32, f32, f32) -> bool + Send + Sync>>` rather than a direct `&WaterState` reference (which cannot be stored in the `Arc<Mutex<ScriptCallContext>>` due to lifetime constraints). The closures are set up by `PlayerApp` before each script update, capturing the current time from the game clock.

- [ ] **Step 4: Build and verify**

Run: `cargo build -p flint-script -p flint-runtime`
Expected: Clean build

- [ ] **Step 5: Commit**

```bash
git add crates/flint-runtime/ crates/flint-script/
git commit -m "feat(script,runtime): add water events, vessel commands, and water query functions"
```

---

## Task 10: Swimming System — Player Water States

**Files:**
- Modify: `crates/flint-player/src/player_app/mod.rs` (PlayerWaterState, swim controller, state transitions)

- [ ] **Step 1: Define `PlayerWaterState` enum**

In `crates/flint-player/src/player_app/mod.rs` (or a new `water.rs` submodule):

```rust
use flint_core::EntityId;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerWaterState {
    OnLand,
    Swimming,
    Underwater,
    OnVessel { vessel_id: EntityId },
}
```

Add `player_water_state: PlayerWaterState` field to `PlayerApp`, defaulting to `OnLand`.

- [ ] **Step 2: Add water state transition logic**

In the fixed update loop, after buoyancy but before character controller:

```rust
// Update player water state
if let Some(ref water) = self.water_state {
    let player_pos = get_player_position(&self.world);
    let feet_y = player_pos.y; // or offset for character height
    let water_height = water.height_at(player_pos.x, player_pos.z, time);

    match self.player_water_state {
        PlayerWaterState::OnLand => {
            if feet_y < water_height {
                self.player_water_state = PlayerWaterState::Swimming;
                self.event_bus.push(GameEvent::PlayerEnteredWater);
            }
        }
        PlayerWaterState::Swimming => {
            if feet_y >= water_height + 0.5 { // hysteresis
                self.player_water_state = PlayerWaterState::OnLand;
                self.event_bus.push(GameEvent::PlayerExitedWater);
            } else if /* dive input pressed */ false {
                self.player_water_state = PlayerWaterState::Underwater;
                self.event_bus.push(GameEvent::PlayerSubmerged);
            }
        }
        PlayerWaterState::Underwater => {
            if /* ascend input */ false {
                self.player_water_state = PlayerWaterState::Swimming;
                self.event_bus.push(GameEvent::PlayerSurfaced);
            }
        }
        PlayerWaterState::OnVessel { .. } => { /* handled by vessel system */ }
    }
}
```

- [ ] **Step 3: Implement swim controller**

When `PlayerWaterState::Swimming`:
- Lock player Y to `water.height_at(x, z, time) + swim_offset`
- WASD moves horizontally along the water surface plane
- Player bobs with waves (Y updates each frame)

When `PlayerWaterState::Underwater`:
- Full 3D movement (forward direction from camera look)
- Slow gravity sink + swim force upward on jump
- No character controller ground check

- [ ] **Step 4: Route input based on water state**

In the character controller update section, check `player_water_state`:
- `OnLand` → existing `update_character()` path
- `Swimming` → swim controller
- `Underwater` → underwater swim controller
- `OnVessel` → vessel input routing (throttle/turn)

- [ ] **Step 5: Set underwater flag on renderer**

After water state update, set the renderer's underwater mode:
```rust
let is_submerged = matches!(self.player_water_state, PlayerWaterState::Underwater)
    || camera_pos.y < water_height;
scene_renderer.set_camera_submerged(is_submerged);
```

- [ ] **Step 6: Test in live player**

Run: `cargo run --bin flint -- play demo/water_test.scene.toml --schemas schemas`
Expected: Player can walk to water edge, transition to swimming, dive underwater. Visual transitions work.

- [ ] **Step 7: Commit**

```bash
git add crates/flint-player/
git commit -m "feat(player): add swimming system with OnLand/Swimming/Underwater state machine"
```

---

## Task 11: Vessel Boarding System

**Files:**
- Modify: `crates/flint-player/src/player_app/mod.rs` (process BoardVessel/DisembarkVessel commands, vessel input routing)

- [ ] **Step 1: Process `BoardVessel` ScriptCommand**

In the command processing section (where `ScriptCommand` variants are matched), add:

```rust
ScriptCommand::BoardVessel { vessel_id } => {
    let eid = EntityId::from_raw(vessel_id as u64);
    // Read vessel component for seat_offset, camera_offset
    // Parent player to vessel
    // Set player position to vessel position + seat_offset
    // Disable character controller
    // Switch camera to vessel's camera_offset
    self.player_water_state = PlayerWaterState::OnVessel { vessel_id: eid };
    self.event_bus.push(GameEvent::PlayerBoardedVessel { vessel_id: eid });
}
ScriptCommand::DisembarkVessel => {
    if let PlayerWaterState::OnVessel { vessel_id } = self.player_water_state {
        // Unparent player
        // Place player beside vessel
        // Re-enable character controller
        // Check if player is in water → Swimming, else → OnLand
        self.player_water_state = /* Swimming or OnLand based on position */;
        self.event_bus.push(GameEvent::PlayerDisembarkedVessel);
    }
}
```

- [ ] **Step 2: Vessel input routing**

When `PlayerWaterState::OnVessel { vessel_id }`, route input to the vessel:

```rust
if let PlayerWaterState::OnVessel { vessel_id } = self.player_water_state {
    // Read vessel component (throttle_force, turn_torque, max_speed)
    // Forward/back → apply force along vessel's forward direction
    // Left/right → apply torque around vessel's Y axis
    // Interact → disembark
}
```

- [ ] **Step 3: Test vessel boarding**

This requires a scene with a vessel entity and a player script that calls `board_vessel()` on interact. Create a test script `demo/scripts/raft_interact.rhai`:

```rhai
fn on_interact() {
    let vessel_id = get_entity_id();
    board_vessel(vessel_id);
}
```

Add a raft entity to `demo/water_test.scene.toml` with the vessel archetype and this script.

- [ ] **Step 4: Test in live player**

Run: `cargo run --bin flint -- play demo/water_test.scene.toml --schemas schemas`
Expected: Walk to raft, press interact, board the raft, steer it around, press interact to disembark.

- [ ] **Step 5: Commit**

```bash
git add crates/flint-player/ demo/
git commit -m "feat(player): add vessel boarding, steering, and disembark system"
```

---

## Task 12: Showcase Scene — "Island Cove"

**Files:**
- Create: `demo/island_cove.scene.toml`
- Create: `demo/island_cove/` (heightmap, splat map, textures)
- Create: `demo/scripts/island_cove_hud.rhai` (optional HUD script)

- [ ] **Step 1: Create the showcase scene**

Create `demo/island_cove.scene.toml` with:
- Terrain island (small heightmap, sand/grass/rock splat)
- `[water]` block with 3 wave layers tuned for good visuals
- Raft vessel entity near the shore
- Player entity with character controller
- Directional sun light, warm skybox
- Post-processing: bloom, fog, volumetric

The heightmap can be generated with `flint gen` using an existing terrain spec, or hand-created as a small grayscale PNG.

- [ ] **Step 2: Validate with `flint render`**

Run multiple render passes to validate each feature:

```bash
# Overview of the island
flint render demo/island_cove.scene.toml -o island_overview.png --schemas schemas \
  --distance 50 --pitch 20 --yaw 45 --time 0.0

# Close-up of shore foam
flint render demo/island_cove.scene.toml -o shore_foam.png --schemas schemas \
  --distance 12 --pitch 15 --target 8,0,8 --time 1.5

# Different wave state
flint render demo/island_cove.scene.toml -o waves_moving.png --schemas schemas \
  --distance 30 --pitch 15 --yaw 90 --time 5.0
```

Expected: Visible ocean with waves, shore foam where terrain meets water, depth color transition.

- [ ] **Step 3: Test in live player**

Run: `cargo run --bin flint -- play demo/island_cove.scene.toml --schemas schemas`
Expected: Full experience — walk around island, swim in ocean, board raft, sail around, dive underwater.

- [ ] **Step 4: Validate `flint edit` integration**

Run: `cargo run --bin flint -- edit demo/island_cove.scene.toml --schemas schemas`
Expected: Interactive scene viewer shows water rendering. Confirm water renders in the live preview with hot-reload.

**Audio note:** Water audio (ambient ocean, splash, underwater filter, hull sounds) is script-driven using the existing `audio_source` system — no engine changes needed. Audio scripts can be added to the showcase scene's HUD controller in a follow-up pass once sound assets are available. This is not blocking for the water system itself.

- [ ] **Step 5: Commit**

```bash
git add demo/island_cove*
git commit -m "feat(demo): add Island Cove showcase scene for water system"
```

---

## Task 13: Final Polish & Cleanup

- [ ] **Step 1: Run `cargo clippy` on all crates**

Run: `cargo clippy`
Fix any warnings in new code.

- [ ] **Step 2: Run `cargo fmt`**

Run: `cargo fmt --check`
Fix formatting issues.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass across all crates.

- [ ] **Step 4: Verify `flint render` validation loop works end-to-end**

Run the showcase render commands from the spec to confirm deterministic, reproducible output.

- [ ] **Step 5: Commit cleanup**

```bash
git add -A
git commit -m "style(water): run clippy and fmt on water system code"
```
