# Grass Debug Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a runtime egui debug panel for tweaking grass rendering parameters with live preview, TOML copy, and commit-to-file.

**Architecture:** New `flint-debug-ui` crate with a `DebugPanel` trait and `GrassDebugPanel` implementation. The player app hosts panels generically, toggling them via F-keys. Grass config changes are pushed to `SceneRenderer` each frame; density changes trigger buffer reallocation. Scene file patching uses the existing `SceneDocument` (toml_edit) API.

**Tech Stack:** Rust, egui 0.30, wgpu 23, toml/toml_edit, flint-terrain `GrassConfig`, flint-scene `SceneDocument`

**Spec:** `docs/superpowers/specs/2026-03-21-grass-debug-overlay-design.md`

---

## File Map

| File | Responsibility |
|---|---|
| **Create:** `crates/flint-debug-ui/Cargo.toml` | Crate manifest |
| **Create:** `crates/flint-debug-ui/src/lib.rs` | `DebugPanel` trait + module exports |
| **Create:** `crates/flint-debug-ui/src/grass_panel.rs` | `GrassDebugPanel` struct + UI + TOML export |
| **Modify:** `Cargo.toml` (workspace root) | Add `flint-debug-ui` to members, default-members, workspace.dependencies |
| **Modify:** `crates/flint-player/Cargo.toml` | Add `flint-debug-ui` dependency |
| **Modify:** `crates/flint-render/src/scene_renderer/mod.rs` | Add `set_grass_config()` and `reload_grass_config()` |
| **Modify:** `crates/flint-player/src/player_app/mod.rs` | F3 toggle, panel storage, render integration, cursor management |

---

## Task 1: Create `flint-debug-ui` crate with `DebugPanel` trait

**Files:**
- Create: `crates/flint-debug-ui/Cargo.toml`
- Create: `crates/flint-debug-ui/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create `crates/flint-debug-ui/Cargo.toml`**

```toml
[package]
name = "flint-debug-ui"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
flint-terrain = { workspace = true }
flint-scene = { workspace = true }
egui = { workspace = true }
toml = { workspace = true }
tracing = { workspace = true }
```

- [ ] **Step 2: Create `crates/flint-debug-ui/src/lib.rs` with `DebugPanel` trait**

```rust
mod grass_panel;

pub use grass_panel::GrassDebugPanel;

/// Common interface for debug overlay panels.
/// The player app holds `Vec<Box<dyn DebugPanel>>` and renders them generically.
pub trait DebugPanel {
    /// Panel identifier used as egui ID and display title.
    fn name(&self) -> &str;

    /// Render the panel contents into the provided egui Ui.
    fn ui(&mut self, ui: &mut egui::Ui);

    /// Whether the panel is currently visible.
    fn is_open(&self) -> bool;

    /// Toggle visibility.
    fn toggle(&mut self);

    /// Returns true if the panel has unapplied changes.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag after changes have been applied by the host.
    fn clear_dirty(&mut self);

    /// Downcast support for accessing concrete panel types.
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
```

- [ ] **Step 3: Add `flint-debug-ui` to workspace `Cargo.toml`**

In the root `Cargo.toml`:

1. Add `"crates/flint-debug-ui",` to `members` array (after line 22, `flint-terrain`)
2. Add `"crates/flint-debug-ui",` to `default-members` array (after line 49, `flint-terrain`)
3. Add to `[workspace.dependencies]` (after line 158, `flint-procgen-ai`):
   ```toml
   flint-debug-ui = { path = "crates/flint-debug-ui" }
   ```

- [ ] **Step 4: Create stub `grass_panel.rs` so crate compiles**

Create `crates/flint-debug-ui/src/grass_panel.rs` with a minimal struct:

```rust
use std::path::PathBuf;
use flint_terrain::GrassConfig;
use crate::DebugPanel;

pub struct GrassDebugPanel {
    config: GrassConfig,
    original: GrassConfig,
    scene_path: PathBuf,
    terrain_entity_name: String,
    open: bool,
    dirty: bool,
    density_changed: bool,
}

impl GrassDebugPanel {
    pub fn new(config: GrassConfig, scene_path: PathBuf, terrain_entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            terrain_entity_name,
            open: false,
            dirty: false,
            density_changed: false,
        }
    }

    /// Read-only access to the working config for the player to push to the renderer.
    pub fn config(&self) -> &GrassConfig {
        &self.config
    }

    /// Whether the density field specifically changed (requires buffer reallocation).
    pub fn density_changed(&self) -> bool {
        self.density_changed
    }

    /// Clear the density_changed flag after the player has handled reallocation.
    pub fn clear_density_changed(&mut self) {
        self.density_changed = false;
    }
}

impl DebugPanel for GrassDebugPanel {
    fn name(&self) -> &str { "Grass Debug" }

    fn ui(&mut self, _ui: &mut egui::Ui) {
        // TODO: implement in Task 3
    }

    fn is_open(&self) -> bool { self.open }
    fn toggle(&mut self) { self.open = !self.open; }
    fn is_dirty(&self) -> bool { self.dirty }
    fn clear_dirty(&mut self) { self.dirty = false; }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
```

- [ ] **Step 5: Verify crate compiles**

Run: `cargo build -p flint-debug-ui`
Expected: Compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/flint-debug-ui/ Cargo.toml
git commit -m "feat: add flint-debug-ui crate with DebugPanel trait"
```

---

## Task 2: Add `set_grass_config` and `reload_grass_config` to SceneRenderer

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs`

- [ ] **Step 1: Add `set_grass_config` method**

Add after the `unload_grass` method (after line 1135 in `crates/flint-render/src/scene_renderer/mod.rs`):

```rust
    /// Update grass config without buffer reallocation.
    /// Compute and render passes read `grass_config` fresh each frame to build
    /// uniforms, so changes take effect on the next `render()` call.
    pub fn set_grass_config(&mut self, config: flint_terrain::GrassConfig) {
        self.grass_config = Some(config);
    }
```

- [ ] **Step 2: Add `reload_grass_config` method**

Add immediately after `set_grass_config`:

```rust
    /// Reallocate the grass instance buffer for a new density value,
    /// reusing existing heightmap/splat GPU textures.
    /// Call this when `GrassConfig.density` changes (affects buffer capacity).
    pub fn reload_grass_config(
        &mut self,
        device: &wgpu::Device,
        config: flint_terrain::GrassConfig,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.max_instances(self.grass_terrain_width, self.grass_terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Reallocate instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Reallocate counter buffer
        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Counter Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Reallocate staging buffer
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Staging Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Recreate compute storage bind group (binds instance buffer at binding 0)
        let compute_storage_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.compute_storage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: counter_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Grass Compute Storage Bind Group"),
            });

        // Recreate render instance bind group (binds instance buffer at binding 0)
        let entity_buffer = self.grass_entity_buffer.as_ref().expect("grass entity buffer");
        let render_instance_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.render_instance_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: entity_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Grass Render Instance Bind Group"),
            });

        // Update stored state
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_counter_buffer = Some(counter_buffer);
        self.grass_staging_buffer = Some(staging_buffer);
        self.grass_compute_storage_bind_group = Some(compute_storage_bind_group);
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_config = Some(config);

        tracing::info!(
            "Grass reloaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Read-only access to the current grass config (if loaded).
    pub fn grass_config(&self) -> Option<&flint_terrain::GrassConfig> {
        self.grass_config.as_ref()
    }
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p flint-render`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/scene_renderer/mod.rs
git commit -m "feat(render): add set_grass_config and reload_grass_config to SceneRenderer"
```

---

## Task 3: Implement GrassDebugPanel UI

**Files:**
- Modify: `crates/flint-debug-ui/src/grass_panel.rs`

- [ ] **Step 1: Implement the full `ui` method with all sections**

Replace the `ui` method stub in `grass_panel.rs` with the complete panel implementation. The panel needs these helper functions and the full `ui` body:

```rust
use std::path::PathBuf;
use flint_terrain::GrassConfig;
use flint_scene::SceneDocument;
use crate::DebugPanel;

/// Helper: render a labeled f32 DragValue slider. Returns true if value changed.
fn drag_f32(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f32, range: std::ops::RangeInclusive<f32>) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    *value != before
}

/// Helper: render a labeled [f32; 3] as 3 DragValues. Returns true if any changed.
fn drag_vec3(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3], speed: f32, range: std::ops::RangeInclusive<f32>) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        for (i, ch) in ["R", "G", "B"].iter().enumerate() {
            ui.add(
                egui::DragValue::new(&mut value[i])
                    .speed(speed)
                    .range(range.clone())
                    .max_decimals(3)
                    .prefix(format!("{}: ", ch)),
            );
        }
    });
    *value != before
}

/// Helper: render a labeled [f32; 3] for XYZ direction. Returns true if any changed.
fn drag_xyz(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3], speed: f32, range: std::ops::RangeInclusive<f32>) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        for (i, ch) in ["X", "Y", "Z"].iter().enumerate() {
            ui.add(
                egui::DragValue::new(&mut value[i])
                    .speed(speed)
                    .range(range.clone())
                    .max_decimals(3)
                    .prefix(format!("{}: ", ch)),
            );
        }
    });
    *value != before
}
```

Then the full `DebugPanel::ui` implementation:

```rust
impl DebugPanel for GrassDebugPanel {
    fn name(&self) -> &str { "Grass Debug" }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Grass Parameters");
        ui.separator();

        // Enable
        let mut changed = false;
        if ui.checkbox(&mut self.config.enabled, "Enabled").changed() {
            changed = true;
        }
        ui.separator();

        // Distribution
        egui::CollapsingHeader::new("Distribution")
            .default_open(true)
            .show(ui, |ui| {
                let prev_density = self.config.density;
                if drag_f32(ui, "Density", &mut self.config.density, 0.1, 0.1..=50.0) {
                    changed = true;
                    if self.config.density != prev_density {
                        self.density_changed = true;
                    }
                }
                changed |= drag_f32(ui, "Max Distance", &mut self.config.max_distance, 1.0, 10.0..=500.0);
                changed |= drag_f32(ui, "Fade Start", &mut self.config.fade_start, 1.0, 5.0..=500.0);
                changed |= drag_f32(ui, "Density Threshold", &mut self.config.density_threshold, 0.01, 0.0..=1.0);
            });

        // Blade Geometry
        egui::CollapsingHeader::new("Blade Geometry")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(ui, "Width", &mut self.config.blade_width, 0.005, 0.01..=1.0);
                changed |= drag_f32(ui, "Height", &mut self.config.blade_height, 0.01, 0.01..=2.0);
                changed |= drag_f32(ui, "Height Variation", &mut self.config.height_variation, 0.01, 0.0..=1.0);
            });

        // Colors
        egui::CollapsingHeader::new("Colors")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_vec3(ui, "Base", &mut self.config.color_base, 0.005, 0.0..=1.0);
                changed |= drag_vec3(ui, "Tip", &mut self.config.color_tip, 0.005, 0.0..=1.0);
                changed |= drag_vec3(ui, "Dry", &mut self.config.color_dry, 0.005, 0.0..=1.0);
                changed |= drag_f32(ui, "Dry Amount", &mut self.config.dry_amount, 0.01, 0.0..=1.0);
            });

        // Wind
        egui::CollapsingHeader::new("Wind")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_xyz(ui, "Direction", &mut self.config.wind_direction, 0.01, -1.0..=1.0);
                changed |= drag_f32(ui, "Speed", &mut self.config.wind_speed, 0.05, 0.0..=10.0);
                changed |= drag_f32(ui, "Strength", &mut self.config.wind_strength, 0.005, 0.0..=1.0);
            });

        // Bend
        egui::CollapsingHeader::new("Bend")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(ui, "Radius", &mut self.config.bend_radius, 0.1, 0.0..=20.0);
                changed |= drag_f32(ui, "Strength", &mut self.config.bend_strength, 0.01, 0.0..=2.0);
            });

        if changed {
            self.dirty = true;
        }

        ui.separator();

        // Bottom toolbar
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                // Check density BEFORE overwriting config
                let density_was_different = self.config.density != self.original.density;
                self.config = self.original.clone();
                self.dirty = true;
                self.density_changed = density_was_different;
            }

            if ui.button("Copy TOML").clicked() {
                let snippet = self.format_toml();
                ui.output_mut(|o| o.copied_text = snippet);
            }

            if ui.button("Commit to File").clicked() {
                if let Err(e) = self.commit_to_file() {
                    tracing::error!("Failed to commit grass config: {}", e);
                }
            }
        });
    }

    fn is_open(&self) -> bool { self.open }
    fn toggle(&mut self) { self.open = !self.open; }
    fn is_dirty(&self) -> bool { self.dirty }
    fn clear_dirty(&mut self) { self.dirty = false; }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
```

- [ ] **Step 2: Implement `format_toml` and `commit_to_file` methods**

Add these private methods to `GrassDebugPanel`:

```rust
impl GrassDebugPanel {
    // ... existing public methods ...

    /// Format current config as "grass.*" TOML keys for pasting under [entities.<name>.terrain].
    fn format_toml(&self) -> String {
        let c = &self.config;
        format!(
            r#""grass.enabled" = {}
"grass.density" = {:.1}
"grass.max_distance" = {:.1}
"grass.fade_start" = {:.1}
"grass.blade_width" = {}
"grass.blade_height" = {}
"grass.height_variation" = {}
"grass.color_base" = [{}, {}, {}]
"grass.color_tip" = [{}, {}, {}]
"grass.color_dry" = [{}, {}, {}]
"grass.dry_amount" = {}
"grass.wind_direction" = [{}, {}, {}]
"grass.wind_speed" = {}
"grass.wind_strength" = {}
"grass.bend_radius" = {}
"grass.bend_strength" = {}
"grass.density_threshold" = {}"#,
            c.enabled,
            c.density, c.max_distance, c.fade_start,
            c.blade_width, c.blade_height, c.height_variation,
            c.color_base[0], c.color_base[1], c.color_base[2],
            c.color_tip[0], c.color_tip[1], c.color_tip[2],
            c.color_dry[0], c.color_dry[1], c.color_dry[2],
            c.dry_amount,
            c.wind_direction[0], c.wind_direction[1], c.wind_direction[2],
            c.wind_speed, c.wind_strength,
            c.bend_radius, c.bend_strength,
            c.density_threshold,
        )
    }

    /// Patch the source scene file with the current grass config values.
    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;

        let entity = &self.terrain_entity_name;
        let comp = "terrain";

        // Helper to convert f32 to toml::Value
        let f = |v: f32| toml::Value::Float(v as f64);
        let b = |v: bool| toml::Value::Boolean(v);
        let v3 = |arr: [f32; 3]| {
            toml::Value::Array(
                arr.iter()
                    .map(|&x| toml::Value::Float(x as f64))
                    .collect(),
            )
        };

        doc.patch_field(entity, comp, "grass.enabled", &b(self.config.enabled))?;
        doc.patch_field(entity, comp, "grass.density", &f(self.config.density))?;
        doc.patch_field(entity, comp, "grass.max_distance", &f(self.config.max_distance))?;
        doc.patch_field(entity, comp, "grass.fade_start", &f(self.config.fade_start))?;
        doc.patch_field(entity, comp, "grass.blade_width", &f(self.config.blade_width))?;
        doc.patch_field(entity, comp, "grass.blade_height", &f(self.config.blade_height))?;
        doc.patch_field(entity, comp, "grass.height_variation", &f(self.config.height_variation))?;
        doc.patch_field(entity, comp, "grass.color_base", &v3(self.config.color_base))?;
        doc.patch_field(entity, comp, "grass.color_tip", &v3(self.config.color_tip))?;
        doc.patch_field(entity, comp, "grass.color_dry", &v3(self.config.color_dry))?;
        doc.patch_field(entity, comp, "grass.dry_amount", &f(self.config.dry_amount))?;
        doc.patch_field(entity, comp, "grass.wind_direction", &v3(self.config.wind_direction))?;
        doc.patch_field(entity, comp, "grass.wind_speed", &f(self.config.wind_speed))?;
        doc.patch_field(entity, comp, "grass.wind_strength", &f(self.config.wind_strength))?;
        doc.patch_field(entity, comp, "grass.bend_radius", &f(self.config.bend_radius))?;
        doc.patch_field(entity, comp, "grass.bend_strength", &f(self.config.bend_strength))?;
        doc.patch_field(entity, comp, "grass.density_threshold", &f(self.config.density_threshold))?;

        doc.save(&self.scene_path)?;
        tracing::info!("Grass config committed to {:?}", self.scene_path);
        Ok(())
    }
}
```

- [ ] **Step 3: Verify crate compiles**

Run: `cargo build -p flint-debug-ui`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/flint-debug-ui/src/grass_panel.rs
git commit -m "feat(debug-ui): implement GrassDebugPanel with sliders, TOML copy, and commit-to-file"
```

---

## Task 4: Integrate debug panels into player app

**Files:**
- Modify: `crates/flint-player/Cargo.toml`
- Modify: `crates/flint-player/src/player_app/mod.rs`

- [ ] **Step 1: Add `flint-debug-ui` dependency to player**

In `crates/flint-player/Cargo.toml`, add after line 31 (`flint-procgen`):

```toml
flint-debug-ui = { workspace = true }
```

- [ ] **Step 2: Add debug panel storage to PlayerApp struct**

In `crates/flint-player/src/player_app/mod.rs`, add a field after line 179 (`terrain` field):

```rust
    // Debug overlay panels (F3 toggle)
    debug_panels: Vec<Box<dyn flint_debug_ui::DebugPanel>>,
```

Initialize it in `PlayerApp::new()` (in the struct literal, alongside other field initializations):

```rust
    debug_panels: Vec::new(),
```

- [ ] **Step 3: Create the grass panel during scene loading**

In `crates/flint-player/src/player_app/scene_loading.rs`, the `load_terrain_from_world_inner` function iterates entities and loads terrain. The terrain entity name is available as `entity.name` (the key from `[entities.<name>]`). Thread it out by returning it alongside the existing return values.

Specifically:
1. In `load_terrain_from_world_inner`, capture the terrain entity's name when it's found
2. Return it as an additional value (e.g., change the return type or add an out parameter)
3. In the calling code in `mod.rs`, after `load_grass` is called, construct the debug panel:

```rust
// After load_grass call, if grass is enabled:
if grass_config.enabled {
    use flint_debug_ui::GrassDebugPanel;
    // terrain_entity_name came from load_terrain_from_world_inner
    let panel = GrassDebugPanel::new(
        grass_config.clone(),
        std::path::PathBuf::from(&self.scene_path),
        terrain_entity_name.to_string(),
    );
    self.debug_panels.push(Box::new(panel));
}
```

Also clear `debug_panels` at the start of scene loading (before terrain load) to handle scene transitions:
```rust
self.debug_panels.clear();
```

- [ ] **Step 4: Add F3 key handler**

In `crates/flint-player/src/player_app/mod.rs`, in the debug key match block (after the F1 handler at line 1943, before F4 at line 1945), add:

```rust
KeyCode::F3 => {
    // Toggle grass debug panel
    let has_grass_panel = self.debug_panels.iter().any(|p| p.name() == "Grass Debug");
    if has_grass_panel {
        for panel in &mut self.debug_panels {
            if panel.name() == "Grass Debug" {
                panel.toggle();
                if panel.is_open() {
                    self.release_cursor();
                } else {
                    // Re-capture if player entity exists
                    if self.physics.has_player_entity() {
                        self.capture_cursor();
                    }
                }
            }
        }
    } else {
        tracing::info!("No terrain with grass in current scene");
    }
}
```

- [ ] **Step 5: Render debug panels in egui**

In `crates/flint-player/src/player_app/mod.rs`, modify the `render_hud` method. Change the `egui_ctx.run()` closure (line 1207) to include debug panel rendering before script draw commands:

Use `std::mem::take` + restore to avoid borrow conflicts (same pattern as the existing `draw_commands` extraction at line 1204):

```rust
let draw_commands = std::mem::take(&mut self.draw_commands);
let mut debug_panels = std::mem::take(&mut self.debug_panels);
let ui_textures = &self.ui_textures;

let full_output = self.egui_ctx.run(raw_input, |ctx| {
    // Debug overlay panels
    for panel in debug_panels.iter_mut() {
        if panel.is_open() {
            egui::SidePanel::right(panel.name())
                .default_width(280.0)
                .show(ctx, |ui| {
                    panel.ui(ui);
                });
        }
    }
    render_draw_commands(ctx, &draw_commands, ui_textures);
});

self.draw_commands = draw_commands;
self.debug_panels = debug_panels;
```

- [ ] **Step 6: Push dirty config to renderer each frame**

In the player's update loop (before `render()` is called), add the per-frame config push. Find where post-process overrides are applied (around lines 864-892) and add after that block:

```rust
// Push debug panel grass config changes to renderer
if let Some(renderer) = &mut self.scene_renderer {
    for panel in &mut self.debug_panels {
        if panel.name() == "Grass Debug" && panel.is_dirty() {
            // Single mutable downcast for both reading and clearing
            let grass_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::GrassDebugPanel>()
                .unwrap();
            if grass_panel.density_changed() {
                if let Some(context) = &self.render_context {
                    renderer.reload_grass_config(&context.device, grass_panel.config().clone());
                }
                grass_panel.clear_density_changed();
            } else {
                renderer.set_grass_config(grass_panel.config().clone());
            }
            panel.clear_dirty();
        }
    }
}
```

For the downcast to work, add `as_any` / `as_any_mut` methods to the `DebugPanel` trait in `crates/flint-debug-ui/src/lib.rs`:

```rust
pub trait DebugPanel {
    // ... existing methods ...

    /// Downcast support for accessing concrete panel types.
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
```

These were already included in the trait definition in Task 1 Step 2.

- [ ] **Step 7: Gate mouse look when panel is open**

In the Escape key handler (line 1922-1930), also prevent re-capturing cursor while a debug panel is open. Add a check:

```rust
// In the Escape handler, before re-capture:
let any_panel_open = self.debug_panels.iter().any(|p| p.is_open());
```

Use this to prevent auto-recapture when clicking back into the window while a panel is open. In the mouse click handler that calls `capture_cursor()` (search for the click-to-recapture logic), gate it:

```rust
if !self.debug_panels.iter().any(|p| p.is_open()) {
    self.capture_cursor();
}
```

- [ ] **Step 8: Verify full build**

Run: `cargo build`
Expected: Full workspace compiles with no errors.

- [ ] **Step 9: Commit**

```bash
git add crates/flint-player/ crates/flint-debug-ui/
git commit -m "feat(player): integrate grass debug panel with F3 toggle and live preview"
```

---

## Task 5: Manual testing and polish

**Files:**
- No new files — testing the integration

- [ ] **Step 1: Run the rolling meadow scene**

Run: `cargo run --bin flint -- play demo/rolling_meadow.scene.toml --schemas schemas`
Expected: Scene loads normally with grass rendering.

- [ ] **Step 2: Test F3 toggle**

Press F3 while running. Expected:
- Right side panel appears with "Grass Parameters" heading
- Cursor is released (visible, not captured)
- Camera stops rotating from mouse movement

Press F3 again. Expected:
- Panel closes
- Cursor is recaptured
- Camera look resumes

- [ ] **Step 3: Test parameter tweaking**

With panel open:
- Drag "Blade Height" slider — grass blades should grow/shrink in real-time
- Change "Color Base" RGB values — grass color should update immediately
- Change "Wind Speed" — wind animation speed should change
- Change "Density" — may briefly stutter as buffer reallocates, then render with new density

- [ ] **Step 4: Test Copy TOML**

Click "Copy TOML" button. Paste into a text editor. Expected: Valid TOML snippet with `"grass.*"` keys.

- [ ] **Step 5: Test Commit to File**

1. Change a visible parameter (e.g., blade height to 0.8)
2. Click "Commit to File"
3. Open `demo/rolling_meadow.scene.toml` — verify the `"grass.blade_height"` value is updated
4. Verify comments and non-grass content are preserved

- [ ] **Step 6: Test Reset**

1. Change several parameters
2. Click "Reset" — all values should snap back to the scene-load values
3. Grass should visually revert

- [ ] **Step 7: Test edge case — scene without grass**

Run a scene without terrain/grass (e.g., a minimal test scene). Press F3. Expected: Console message "No terrain with grass in current scene", no panel appears.

- [ ] **Step 8: Final commit if any polish needed**

```bash
git add -A
git commit -m "fix(debug-ui): polish grass debug panel after manual testing"
```
