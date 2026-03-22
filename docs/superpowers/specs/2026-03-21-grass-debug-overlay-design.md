# Grass Debug Overlay — Design Spec

## Summary

A runtime debug overlay for tweaking grass rendering parameters in the Flint player. An egui side panel (toggled via F3) provides sliders and color editors for all grass config fields, with live preview and the ability to copy settings as TOML or commit them directly back to the source scene file.

## Goals

- Tune grass visuals at runtime without edit-save-reload cycles
- Copy final values as TOML or write them back to the scene file
- Establish a reusable `DebugPanel` trait for future debug overlays

## Non-Goals

- Terrain geometry parameters (width, depth, height_scale, chunk_resolution)
- Script API for grass parameter overrides
- Undo/redo history within the debug panel
- `density_source` and `density_layer` fields (these control which splat map channel drives density placement — rarely tweaked visually, better edited in TOML directly)

## Architecture

### New Crate: `flint-debug-ui`

Location: `crates/flint-debug-ui/`

Dependencies:
- `flint-terrain` — `GrassConfig` struct
- `flint-scene` — `SceneDocument` for comment-preserving TOML patching
- `egui = "0.30.0"` — UI widgets

### DebugPanel Trait

```rust
pub trait DebugPanel {
    fn name(&self) -> &str;
    fn ui(&mut self, ui: &mut egui::Ui);
    fn is_open(&self) -> bool;
    fn toggle(&mut self);
    /// Returns true if the panel has unapplied changes.
    fn is_dirty(&self) -> bool;
    /// Clear the dirty flag after changes have been applied.
    fn clear_dirty(&mut self);
}
```

`flint-player` holds panels as `Vec<Box<dyn DebugPanel>>` and renders them generically. For the grass panel specifically, the player downcasts to `GrassDebugPanel` to access `config()` and `density_changed()` — this is pragmatic since the player constructs the concrete type and needs type-specific data (the `GrassConfig` value) to push to the renderer. The trait handles the common lifecycle (open/close, dirty detection, rendering).

### GrassDebugPanel

Implements `DebugPanel`. Stores:
- `config: GrassConfig` — working copy, mutated by UI widgets
- `original: GrassConfig` — snapshot from when panel was opened, for reset
- `scene_path: PathBuf` — source scene file for commit-to-file
- `terrain_entity_name: String` — TOML entity key (e.g., "ground") for patching
- `open: bool` — visibility toggle
- `dirty: bool` — tracks whether config has changed since last applied
- `density_changed: bool` — tracks whether the `density` field specifically changed (requires instance buffer reallocation); `density_threshold` changes do NOT set this flag

## Panel Layout

egui `SidePanel::right` with `.default_width(280.0)`, collapsible sections (right side is consistent with the viewer's inspector panel convention):

### Enable
| Field | Widget |
|---|---|
| `enabled` | Checkbox |

### Distribution
| Field | Widget | Range |
|---|---|---|
| `density` | DragValue | 0.1 .. 50.0 |
| `max_distance` | DragValue | 10.0 .. 500.0 |
| `fade_start` | DragValue | 5.0 .. 500.0 |
| `density_threshold` | DragValue | 0.0 .. 1.0 |

### Blade Geometry
| Field | Widget | Range |
|---|---|---|
| `blade_width` | DragValue | 0.01 .. 1.0 |
| `blade_height` | DragValue | 0.01 .. 2.0 |
| `height_variation` | DragValue | 0.0 .. 1.0 |

### Colors
| Field | Widget |
|---|---|
| `color_base` | 3x DragValue (RGB, 0.0..1.0) |
| `color_tip` | 3x DragValue (RGB, 0.0..1.0) |
| `color_dry` | 3x DragValue (RGB, 0.0..1.0) |
| `dry_amount` | DragValue (0.0..1.0) |

### Wind
| Field | Widget | Range |
|---|---|---|
| `wind_direction` | 3x DragValue (XYZ) | -1.0 .. 1.0 |
| `wind_speed` | DragValue | 0.0 .. 10.0 |
| `wind_strength` | DragValue | 0.0 .. 1.0 |

### Bend
| Field | Widget | Range |
|---|---|---|
| `bend_radius` | DragValue | 0.0 .. 20.0 |
| `bend_strength` | DragValue | 0.0 .. 2.0 |

### Bottom Toolbar
- **Reset** button — restores `original` config
- **Copy TOML** button — formats config as `"grass.*"` keys, copies to clipboard
- **Commit to File** button — patches grass fields in the source scene TOML and writes back

## Data Flow

### Per-Frame Update

1. Player checks `panel.is_dirty()` (via trait method)
2. If dirty, downcast to `GrassDebugPanel` to read `config()` and `density_changed()`
3. If non-density change: call `SceneRenderer::set_grass_config(config)` — compute and render passes read `grass_config` fresh each frame to build uniforms, so no additional invalidation needed
4. If density changed: call `SceneRenderer::reload_grass_config(config)` which reallocates the instance buffer (capacity depends on density × terrain area) while reusing the existing heightmap/splat GPU textures
5. Call `panel.clear_dirty()` (via trait method)

### Copy TOML

Formats the working `GrassConfig` as a TOML snippet:
```toml
"grass.enabled" = true
"grass.density" = 10.0
"grass.blade_width" = 0.09
...
```
Copies to clipboard via `ui.output_mut(|o| o.copied_text = snippet)`. The snippet is valid for pasting under an existing `[entities.<name>.terrain]` header in a scene TOML file.

### Commit to File

Uses `SceneDocument` from `flint-scene` (backed by `toml_edit`) to patch individual fields while preserving comments and formatting:

1. Load the source scene file as a `SceneDocument`
2. For each grass field, call `patch_field(terrain_entity_name, "terrain", "grass.<field>", value)`
3. Write back via `SceneDocument::save()` — preserves comments, ordering, and non-grass content

## Edge Cases

**No terrain/grass in scene:** The `GrassDebugPanel` is only constructed when a terrain entity with grass enabled is loaded. If F3 is pressed and no grass panel exists, log a message ("No terrain with grass in current scene") and do nothing.

## Player App Integration

### F3 Toggle

F3 is currently unused. Sits between F1 (debug mode) and F4 (shadows). Pressing F3 toggles the grass debug panel (if one exists for the current scene).

### egui Rendering

Inside the player's `egui_ctx.run()` closure in `render_hud`, before script draw commands:
```rust
for panel in &mut self.debug_panels {
    if panel.is_open() {
        egui::SidePanel::right(panel.name()).show(ctx, |ui| {
            panel.ui(ui);
        });
    }
}
```

### Mouse Capture

The player uses `DeviceEvent::MouseMotion` raw deltas for camera look, which bypass egui. The cursor must be released (Escape) before interacting with the debug panel — this is the natural flow since the panel requires a visible cursor. Pressing F3 to open the panel should auto-release cursor capture. Closing the panel (F3 again) re-captures the cursor. Additionally, gate character controller mouse look on `!ctx.wants_pointer_input()` as a safety check.

## SceneRenderer API Changes

Two new methods:

```rust
/// Update grass config without buffer reallocation.
/// Compute and render passes read grass_config fresh each frame,
/// so changes take effect on the next render() call.
pub fn set_grass_config(&mut self, config: GrassConfig) {
    self.grass_config = Some(config);
}

/// Reallocate the grass instance buffer for a new density value,
/// reusing the existing heightmap/splat GPU textures.
/// Uses self.grass_terrain_width / self.grass_terrain_depth (already stored from load_grass).
pub fn reload_grass_config(&mut self, device: &wgpu::Device, config: GrassConfig) {
    // Reallocate instance buffer based on config.max_instances(self.grass_terrain_width, self.grass_terrain_depth)
    // Recreate compute_storage_bind_group (binds instance buffer at binding 0)
    // Recreate render_instance_bind_group (binds instance buffer at binding 0)
    // Reuse: grass_compute_texture_bind_group, compute_uniform_bind_group,
    //        render_uniform_bind_group, entity_buffer, render_uniform_buffer
    // Store new config
}
```

`set_grass_config` handles the common case (all fields except density). `reload_grass_config` handles density changes that affect instance buffer capacity — it reads terrain dimensions from `self` (already stored from `load_grass`), reallocates the instance buffer, and recreates the two bind groups that reference it, while reusing all other GPU resources. Note: `density_threshold` does NOT require reallocation (it's a compute uniform that filters instances at dispatch time), only the `density` field affects buffer capacity via `max_instances()`.

## File Changes Summary

| File | Change |
|---|---|
| `Cargo.toml` (workspace) | Add `flint-debug-ui` to members + default-members |
| `crates/flint-debug-ui/Cargo.toml` | New crate manifest |
| `crates/flint-debug-ui/src/lib.rs` | `DebugPanel` trait, module exports |
| `crates/flint-debug-ui/src/grass_panel.rs` | `GrassDebugPanel` implementation |
| `crates/flint-player/Cargo.toml` | Add `flint-debug-ui` dependency |
| `crates/flint-player/src/player_app/mod.rs` | F3 handler, debug panel storage, render integration, per-frame config push |
| `crates/flint-render/src/scene_renderer/mod.rs` | Add `set_grass_config()` and `reload_grass_config()` methods |
