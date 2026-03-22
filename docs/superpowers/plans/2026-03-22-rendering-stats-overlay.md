# Rendering Stats Overlay Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Unity-style rendering statistics overlay (FPS, draw calls, triangles, per-system breakdown, shadow stats) toggled with F2 in both the player and viewer.

**Architecture:** A `RenderStats` data struct in `flint-render` populated by `SceneRenderer::collect_stats()`. The egui overlay rendering lives in the consumer crates (`flint-player` and `flint-viewer`). To free F2 in the viewer, the wireframe overlay is folded into the F1 debug mode cycle as a new `DebugMode::WireframeOverlay` variant.

**Tech Stack:** Rust, wgpu 23, egui, flint-render / flint-player / flint-viewer crates

**Spec:** `docs/superpowers/specs/2026-03-22-rendering-stats-overlay-design.md`

---

## Chunk 1: RenderStats struct and format_count helper

### Task 1: Create `render_stats.rs` with tests

**Files:**
- Create: `crates/flint-render/src/render_stats.rs`
- Modify: `crates/flint-render/src/lib.rs`

- [ ] **Step 1: Create `render_stats.rs` with `RenderStats` struct and `format_count` stub**

```rust
//! Rendering statistics collected per frame

/// All rendering metrics for a single frame.
///
/// The `collect_stats()` method on `SceneRenderer` populates the draw call
/// and triangle fields. Timing (`fps`, `frame_time_ms`) and `resolution`
/// are set by the caller (player or viewer) since they own the timing source.
#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    // Timing (set by caller)
    pub fps: f32,
    pub frame_time_ms: f32,
    // Totals
    pub draw_calls: u32,
    pub triangles: u32,
    // Per-system breakdown
    pub entity_draws: u32,
    pub skinned_draws: u32,
    pub terrain_draws: u32,
    pub terrain_total_chunks: u32,
    pub transparent_draws: u32,
    pub billboard_draws: u32,
    pub particle_draws: u32,
    pub particle_instances: u32,
    pub sprite_batches: u32,
    pub grass_instances: u32,
    pub grass_draw_calls: u32,
    // Shadow pass (estimated)
    pub shadow_draw_calls: u32,
    pub shadow_triangles: u32,
    // Screen info (set by caller)
    pub resolution: [u32; 2],
}

/// Format a count for display: 1234 → "1.2K", 1234567 → "1.2M", 42 → "42"
pub fn format_count(n: u32) -> String {
    todo!()
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/flint-render/src/lib.rs`, add after `pub mod frustum;` (line 17):

```rust
pub mod render_stats;
```

And add after `pub use frustum::Frustum;` (line 66):

```rust
pub use render_stats::{RenderStats, format_count};
```

- [ ] **Step 3: Write tests for `format_count`**

Append to `crates/flint-render/src/render_stats.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_small_numbers() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn format_count_thousands() {
        assert_eq!(format_count(1000), "1.0K");
        assert_eq!(format_count(1234), "1.2K");
        assert_eq!(format_count(9999), "10.0K");
        assert_eq!(format_count(142_300), "142.3K");
    }

    #[test]
    fn format_count_millions() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(1_234_567), "1.2M");
        assert_eq!(format_count(48_000_000), "48.0M");
    }

    #[test]
    fn default_stats_all_zeros() {
        let stats = RenderStats::default();
        assert_eq!(stats.fps, 0.0);
        assert_eq!(stats.draw_calls, 0);
        assert_eq!(stats.triangles, 0);
        assert_eq!(stats.terrain_draws, 0);
        assert_eq!(stats.terrain_total_chunks, 0);
        assert_eq!(stats.resolution, [0, 0]);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p flint-render render_stats -- --nocapture`
Expected: FAIL — `todo!()` panics

- [ ] **Step 5: Implement `format_count`**

Replace `todo!()`:

```rust
pub fn format_count(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p flint-render render_stats -- --nocapture`
Expected: All 4 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/flint-render/src/render_stats.rs crates/flint-render/src/lib.rs
git commit -m "feat(render): add RenderStats struct and format_count helper

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

### Task 2: Add `collect_stats()` to `SceneRenderer`

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs`

- [ ] **Step 1: Add `terrain_total_chunks` and `terrain_visible_chunks` fields to `SceneRenderer`**

In `crates/flint-render/src/scene_renderer/mod.rs`, add after the `terrain_draws` field (line 140):

```rust
    terrain_total_chunks: u32,
    terrain_visible_chunks: u32,
```

Find ALL constructor sites (search for `terrain_draws: Vec::new()` — there are two, around lines 286 and 1384) and add `terrain_total_chunks: 0, terrain_visible_chunks: 0,` after each.

- [ ] **Step 2: Set `terrain_total_chunks` in terrain loading methods**

In `load_terrain()` (around line 429), after clearing terrain draws (`self.terrain_draws.clear();` around line 551), add:

```rust
        self.terrain_total_chunks = chunks.len() as u32;
```

In `load_terrain_from_data()` (around line 693), before the call to `self.reload_terrain_geometry(...)` (around line 842), add:

```rust
        self.terrain_total_chunks = chunks.len() as u32;
```

- [ ] **Step 2b: Count visible terrain chunks during normal pass culling**

In `crates/flint-render/src/scene_renderer/render_passes.rs`, in the `render_normal_pass` terrain loop (around line 715), count visible chunks. The current code is:

```rust
                for draw in &self.terrain_draws {
                    if let Some(ref frustum) = self.camera_frustum {
                        if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                            continue;
                        }
                    }
                    // ... draw call ...
                }
```

Since `render_normal_pass` takes `&self` (immutable), we can't write to `self.terrain_visible_chunks` here. Instead, count visible chunks in `render_to()` (which takes `&mut self`) right after setting the camera frustum. Add after the `self.camera_frustum = Some(...)` line:

```rust
        // Count visible terrain chunks for stats
        self.terrain_visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain_draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain_draws.len() as u32
        };
```

- [ ] **Step 3: Implement `collect_stats()`**

Add this method to the `impl SceneRenderer` block (after `toggle_wireframe_overlay` around line 1624):

```rust
    /// Collect rendering statistics for the current frame.
    ///
    /// Timing (`fps`, `frame_time_ms`) and `resolution` are left at zero —
    /// the caller fills these from their own timing and context.
    pub fn collect_stats(&self) -> crate::render_stats::RenderStats {
        use crate::grass_pipeline::BLADE_INDEX_COUNT;

        let entity_draws = self.entity_draws.len() as u32;
        let skinned_draws = self.skinned_entity_draws.len() as u32;
        let terrain_draws = self.terrain_visible_chunks;
        let transparent_draws =
            (self.transparent_draws.len() + self.transparent_skinned_draws.len()) as u32;
        let billboard_draws = self.billboard_draws.len() as u32;
        let particle_draws = self.particle_draws.len() as u32;
        let sprite_batches = self.sprite2d_batches.len() as u32;
        let grass_draw_calls = if self.grass_instance_count > 0 { 1u32 } else { 0 };

        let draw_calls = entity_draws
            + skinned_draws
            + terrain_draws
            + transparent_draws
            + billboard_draws
            + particle_draws
            + sprite_batches
            + grass_draw_calls;

        // Triangles: sum index_count/3 for types that have it, fixed counts for others
        let mut triangles: u32 = 0;
        for d in &self.entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.skinned_entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_skinned_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.terrain_draws {
            triangles += d.index_count / 3;
        }
        // Billboards: each is a fixed quad (2 triangles)
        triangles += billboard_draws * 2;
        // Particles: instanced quads (2 triangles per instance)
        let particle_instances: u32 = self.particle_draws.iter().map(|d| d.instance_count).sum();
        triangles += particle_instances * 2;
        // Sprites: instanced quads (2 triangles per instance)
        let sprite_instances: u32 =
            self.sprite2d_batches.iter().map(|b| b.instance_count).sum();
        triangles += sprite_instances * 2;
        // Grass: BLADE_INDEX_COUNT indices per instance
        triangles += self.grass_instance_count * BLADE_INDEX_COUNT / 3;

        // Shadow stats: estimate as main pass × CASCADE_COUNT
        let cascade_count = crate::shadow::CASCADE_COUNT as u32;
        let shadow_entity_draws = entity_draws + skinned_draws + terrain_draws;
        let shadow_draw_calls = shadow_entity_draws * cascade_count;
        let shadow_triangles = {
            let mut t: u32 = 0;
            for d in &self.entity_draws {
                t += d.index_count / 3;
            }
            for d in &self.skinned_entity_draws {
                t += d.index_count / 3;
            }
            for d in &self.terrain_draws {
                t += d.index_count / 3;
            }
            t * cascade_count
        };

        crate::render_stats::RenderStats {
            draw_calls,
            triangles,
            entity_draws,
            skinned_draws,
            terrain_draws,
            terrain_total_chunks: self.terrain_total_chunks,
            transparent_draws,
            billboard_draws,
            particle_draws,
            particle_instances,
            sprite_batches,
            grass_instances: self.grass_instance_count,
            grass_draw_calls,
            shadow_draw_calls,
            shadow_triangles,
            ..Default::default()
        }
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p flint-render`
Expected: Success

- [ ] **Step 5: Run all tests**

Run: `cargo test -p flint-render`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/flint-render/src/scene_renderer/mod.rs
git commit -m "feat(render): add collect_stats() for rendering statistics

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

## Chunk 2: Free F2 by folding wireframe overlay into debug mode cycle

### Task 3: Add `WireframeOverlay` to `DebugMode` and remove `wireframe_overlay` bool

**Files:**
- Modify: `crates/flint-render/src/debug.rs`
- Modify: `crates/flint-render/src/scene_renderer/mod.rs`
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs`
- Modify: `crates/flint-cli/src/commands/render.rs` (wireframe overlay flag + debug-mode mapping)
- Modify: `crates/flint-cli/src/commands/gen_preview.rs` (F2 handler)
- Modify: `crates/flint-cli/src/commands/serve.rs` (F2 handler)
- Modify: `crates/flint-cli/src/commands/preview.rs` (DebugMode combo box list)

- [ ] **Step 1: Add `WireframeOverlay` variant to `DebugMode`**

In `crates/flint-render/src/debug.rs`, add the new variant after `Pbr` (line 8):

```rust
    /// Wireframe overlay on top of solid PBR geometry
    WireframeOverlay,
```

Update `next()` (line 25):

```rust
    pub fn next(self) -> Self {
        match self {
            Self::Pbr => Self::WireframeOverlay,
            Self::WireframeOverlay => Self::WireframeOnly,
            Self::WireframeOnly => Self::Normals,
            Self::Normals => Self::Depth,
            Self::Depth => Self::UvChecker,
            Self::UvChecker => Self::Unlit,
            Self::Unlit => Self::MetallicRoughness,
            Self::MetallicRoughness => Self::Pbr,
        }
    }
```

Update `as_u32()` (line 38):

```rust
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Pbr => 0,
            Self::WireframeOverlay => 0, // shader stays PBR, overlay handled by pipeline
            Self::WireframeOnly => 0,
            Self::Normals => 1,
            Self::Depth => 2,
            Self::UvChecker => 3,
            Self::Unlit => 4,
            Self::MetallicRoughness => 5,
        }
    }
```

Update `label()` (line 50):

```rust
    pub fn label(self) -> &'static str {
        match self {
            Self::Pbr => "PBR",
            Self::WireframeOverlay => "Wireframe Overlay",
            Self::WireframeOnly => "Wireframe",
            Self::Normals => "Normals",
            Self::Depth => "Depth",
            Self::UvChecker => "UV Checker",
            Self::Unlit => "Unlit",
            Self::MetallicRoughness => "Metal/Rough",
        }
    }
```

- [ ] **Step 2: Remove `wireframe_overlay` bool from `DebugState`**

In `crates/flint-render/src/debug.rs`, remove line 69:

```rust
    /// Whether wireframe overlay is drawn on top of solid geometry (F2 toggles)
    pub wireframe_overlay: bool,
```

And remove `wireframe_overlay: false,` from the `Default` impl (line 82).

- [ ] **Step 3: Update `SceneRenderer` references to `wireframe_overlay`**

In `crates/flint-render/src/scene_renderer/mod.rs`:

**Line 1939** — change:
```rust
        let need_overlay =
            self.debug_state.wireframe_overlay || self.debug_state.mode == DebugMode::WireframeOnly;
```
to:
```rust
        let need_overlay =
            self.debug_state.mode == DebugMode::WireframeOverlay || self.debug_state.mode == DebugMode::WireframeOnly;
```

**Remove `toggle_wireframe_overlay` method** (lines 1620-1624):
```rust
    pub fn toggle_wireframe_overlay(&mut self) -> bool {
        self.debug_state.wireframe_overlay = !self.debug_state.wireframe_overlay;
        self.debug_state.wireframe_overlay
    }
```

- [ ] **Step 4: Update render pass wireframe overlay check**

In `crates/flint-render/src/scene_renderer/render_passes.rs`, line 959 — change:

```rust
        if self.debug_state.wireframe_overlay {
```

to:

```rust
        if self.debug_state.mode == DebugMode::WireframeOverlay {
```

Add the import at the top of `render_passes.rs` if `DebugMode` isn't already imported — check the existing imports first. It's likely already available via `use crate::debug::DebugMode;` or through `self.debug_state.mode`.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p flint-render`
Expected: Possible warnings about unused `toggle_wireframe_overlay` import in viewer — we'll fix that next.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p flint-render`
Expected: All tests pass

- [ ] **Step 7: Update viewer to remove F2 wireframe handler**

In `crates/flint-viewer/src/app.rs`:

**Remove the F2 handler** (lines 1689-1698 — the `KeyCode::F2` block that calls `toggle_wireframe_overlay()`).

Keep the F3 handler (normal arrows) as-is — normal arrows are a separate overlay, not a debug shading mode.

- [ ] **Step 8: Update flint-cli callers of `toggle_wireframe_overlay`**

Three files in `crates/flint-cli/src/commands/` call `toggle_wireframe_overlay()` and will no longer compile:

**`render.rs`** (~line 182): The `--wireframe-overlay` CLI flag calls `toggle_wireframe_overlay()`. Replace with:
```rust
renderer.set_debug_mode(DebugMode::WireframeOverlay);
```
Also add `"wireframe-overlay" => DebugMode::WireframeOverlay` to the `--debug-mode` string mapping (~line 171-177).

**`gen_preview.rs`** (~line 1339): The F2 handler calls `toggle_wireframe_overlay()`. Replace with a toggle between `WireframeOverlay` and `Pbr`:
```rust
KeyCode::F2 => {
    if let Some(renderer) = &mut self.scene_renderer {
        let mode = if renderer.debug_state().mode == DebugMode::WireframeOverlay {
            DebugMode::Pbr
        } else {
            DebugMode::WireframeOverlay
        };
        renderer.set_debug_mode(mode);
        // ... update_from_world if needed ...
    }
}
```

**`serve.rs`** (~line 439): Same pattern as gen_preview.

- [ ] **Step 9: Update hardcoded DebugMode variant list in `preview.rs`**

In `crates/flint-cli/src/commands/preview.rs` (~line 1332-1340), there's a hardcoded array of all `DebugMode` variants for an egui combo box. Add `DebugMode::WireframeOverlay` between `Pbr` and `WireframeOnly`.

- [ ] **Step 10: Update player — no changes needed**

The player doesn't have an F2 handler currently (F-keys go F1, then F3-F12). No changes needed.

- [ ] **Step 11: Verify full build**

Run: `cargo check`
Expected: Clean build, no errors

- [ ] **Step 12: Commit**

```bash
git add crates/flint-render/src/debug.rs crates/flint-render/src/scene_renderer/mod.rs crates/flint-render/src/scene_renderer/render_passes.rs crates/flint-viewer/src/app.rs crates/flint-cli/src/commands/render.rs crates/flint-cli/src/commands/gen_preview.rs crates/flint-cli/src/commands/serve.rs crates/flint-cli/src/commands/preview.rs
git commit -m "refactor(render): fold wireframe overlay into F1 debug mode cycle

Move wireframe overlay from standalone F2 toggle to DebugMode::WireframeOverlay
in the F1 cycle. Frees F2 for the rendering stats overlay.

Cycle: PBR → Wireframe Overlay → Wireframe Only → Normals → Depth → UV → Unlit → Metal/Rough

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

## Chunk 3: Stats overlay in player and viewer

### Task 4: Add F2 stats overlay to the player

**Files:**
- Modify: `crates/flint-player/src/player_app/mod.rs`

- [ ] **Step 1: Add `show_stats` field and FPS tracking to `PlayerApp`**

In `crates/flint-player/src/player_app/mod.rs`, add to the `PlayerApp` struct (after `debug_panels` around line 182):

```rust
    // Rendering stats overlay (F2 toggle)
    show_stats: bool,
    stats_frame_times: std::collections::VecDeque<f64>,
```

In the constructor (`PlayerApp::new()`, the `Self { ... }` block starting around line 207), add:

```rust
            show_stats: false,
            stats_frame_times: std::collections::VecDeque::new(),
```

- [ ] **Step 2: Add F2 key handler**

In the debug key match block (around line 2007), add a new arm before `KeyCode::F3`:

```rust
                                KeyCode::F2 => {
                                    self.show_stats = !self.show_stats;
                                }
```

- [ ] **Step 3: Add `render_stats_overlay` function**

Add this function at the bottom of `mod.rs` (before the closing of the module, or as a standalone function near the egui rendering code):

```rust
fn render_stats_overlay(ctx: &egui::Context, stats: &flint_render::RenderStats) {
    use flint_render::format_count;

    egui::Area::new(egui::Id::new("render_stats_overlay"))
        .fixed_pos(egui::pos2(
            ctx.screen_rect().right() - 210.0,
            8.0,
        ))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
                ))
                .rounding(egui::Rounding::same(4.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.style_mut().override_font_id =
                        Some(egui::FontId::monospace(11.0));
                    ui.set_min_width(180.0);

                    // Header
                    ui.colored_label(
                        egui::Color32::from_gray(136),
                        egui::RichText::new("RENDERING STATS").size(9.0),
                    );
                    ui.separator();

                    // Core metrics
                    ui.horizontal(|ui| {
                        ui.label("FPS:");
                        ui.colored_label(
                            egui::Color32::from_rgb(74, 222, 128),
                            format!("{:.0}", stats.fps),
                        );
                        ui.colored_label(
                            egui::Color32::from_gray(102),
                            format!("({:.1}ms)", stats.frame_time_ms),
                        );
                    });
                    ui.label(format!("Draw Calls: {}", stats.draw_calls));
                    ui.label(format!("Triangles: {}", format_count(stats.triangles)));

                    ui.separator();

                    // Breakdown
                    ui.label(format!("Entities: {}", stats.entity_draws));
                    if stats.skinned_draws > 0 {
                        ui.label(format!("Skinned: {}", stats.skinned_draws));
                    }
                    ui.label(format!(
                        "Terrain: {}/{} chunks",
                        stats.terrain_draws, stats.terrain_total_chunks
                    ));
                    if stats.transparent_draws > 0 {
                        ui.label(format!("Transparent: {}", stats.transparent_draws));
                    }
                    if stats.billboard_draws > 0 {
                        ui.label(format!("Billboards: {}", stats.billboard_draws));
                    }
                    if stats.particle_draws > 0 {
                        ui.label(format!(
                            "Particles: {} ({} inst)",
                            stats.particle_draws,
                            format_count(stats.particle_instances)
                        ));
                    }
                    if stats.sprite_batches > 0 {
                        ui.label(format!("Sprites: {}", stats.sprite_batches));
                    }
                    if stats.grass_instances > 0 {
                        ui.label(format!(
                            "Grass: {} inst",
                            format_count(stats.grass_instances)
                        ));
                    }

                    ui.separator();

                    // Shadow pass
                    ui.label(format!("Shadow Calls: {}", stats.shadow_draw_calls));
                    ui.label(format!(
                        "Shadow Tris: {}",
                        format_count(stats.shadow_triangles)
                    ));

                    ui.separator();

                    // Resolution
                    ui.colored_label(
                        egui::Color32::from_gray(102),
                        format!("{}x{}", stats.resolution[0], stats.resolution[1]),
                    );
                });
        });
}
```

- [ ] **Step 4: Call the overlay in the egui rendering pass**

In the `egui_ctx.run()` closure (around line 1242), add the stats overlay call after the draw commands (after line 1255 `render_draw_commands(...)`):

```rust
            // Rendering stats overlay (F2)
            if show_stats {
                if let Some(renderer) = &scene_renderer_ref {
                    let mut stats = renderer.collect_stats();
                    // FPS smoothing: rolling average over ~0.5s
                    stats.fps = smoothed_fps;
                    stats.frame_time_ms = if smoothed_fps > 0.0 { 1000.0 / smoothed_fps } else { 0.0 };
                    stats.resolution = resolution;
                    render_stats_overlay(ctx, &stats);
                }
            }
```

Note: The exact integration depends on the borrow structure of the egui closure. The `scene_renderer` is behind `&mut self` which may be borrowed. You'll need to extract `show_stats`, a reference to `scene_renderer`, and the resolution + FPS data before entering the closure, similar to how `draw_commands` is extracted with `std::mem::take`. Specifically:

1. Before the `egui_ctx.run()` call, compute:
   ```rust
   let show_stats = self.show_stats;
   let stats_data = if show_stats {
       self.scene_renderer.as_ref().map(|r| {
           let mut stats = r.collect_stats();
           // FPS: add current delta_time to rolling window, compute average
           self.stats_frame_times.push_back(self.clock.delta_time);
           let cutoff_count = (0.5 / self.clock.delta_time.max(0.001)) as usize;
           while self.stats_frame_times.len() > cutoff_count.max(1) {
               self.stats_frame_times.pop_front();
           }
           let avg_dt: f64 = self.stats_frame_times.iter().sum::<f64>()
               / self.stats_frame_times.len() as f64;
           stats.fps = (1.0 / avg_dt) as f32;
           stats.frame_time_ms = (avg_dt * 1000.0) as f32;
           if let Some(ctx) = &self.render_context {
               stats.resolution = [ctx.config.width, ctx.config.height];
           }
           stats
       })
   } else {
       None
   };
   ```

2. Inside the `egui_ctx.run()` closure, add:
   ```rust
   if let Some(stats) = &stats_data {
       render_stats_overlay(ctx, stats);
   }
   ```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p flint-player`
Expected: Success (may need to adjust borrow patterns — the implementer should work through any borrow checker issues following the pattern above)

- [ ] **Step 6: Commit**

```bash
git add crates/flint-player/src/player_app/mod.rs
git commit -m "feat(player): add F2 rendering stats overlay

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

### Task 5: Add F2 stats overlay to the viewer, replace old RenderStats

**Files:**
- Modify: `crates/flint-viewer/src/app.rs`
- Modify: `crates/flint-viewer/src/panels/render_stats.rs`
- Modify: `crates/flint-viewer/src/panels/mod.rs`

- [ ] **Step 1: Add `show_stats` field to `ViewerApp`**

In `crates/flint-viewer/src/app.rs`, find the `ViewerApp` struct (around line 250) and add:

```rust
    show_stats: bool,
```

Initialize to `false` in both constructors (search for `render_stats: RenderStats::new()` — around lines 328 and 366).

- [ ] **Step 2: Add F2 key handler in the viewer**

The old F2 handler was removed in Task 3. Add a new F2 handler in its place (around line 1689):

```rust
                        PhysicalKey::Code(KeyCode::F2) => {
                            self.show_stats = !self.show_stats;
                        }
```

- [ ] **Step 3: Add `render_stats_overlay` function to the viewer**

Copy the same `render_stats_overlay` function from Task 4 Step 3 into the viewer's `app.rs` (at the bottom of the file, or in a suitable location). It's identical — same egui layout.

- [ ] **Step 4: Wire up the overlay in the viewer's render loop**

Find where `self.render_stats.record_frame()` is called (around line 528) and where `render_stats.ui(ui)` is called (around line 714).

Keep the `record_frame()` call for FPS tracking. Replace the `render_stats.ui(ui)` usage in the inspector panel with the new overlay. Specifically:

1. After `self.render_stats.record_frame()`, if `self.show_stats`, collect stats:
   ```rust
   if self.show_stats {
       if let Some(renderer) = &self.scene_renderer {
           let mut stats = renderer.collect_stats();
           stats.fps = self.render_stats.fps();
           stats.frame_time_ms = if stats.fps > 0.0 { 1000.0 / stats.fps } else { 0.0 };
           if let Some(ctx) = &self.render_context {
               stats.resolution = [ctx.config.width, ctx.config.height];
           }
           render_stats_overlay(&self.egui_ctx, &stats);
       }
   }
   ```

   Note: The existing `RenderStats` in the viewer tracks FPS via `record_frame()`. Add a `pub fn fps(&self) -> f32` getter if one doesn't exist (check `render_stats.rs` — it has `self.fps` as a field, but `ui()` reads it directly; we need a public getter).

2. Remove the old `render_stats.ui(ui)` call from the inspector panel (around line 714).

- [ ] **Step 5: Add `fps()` getter to viewer's RenderStats if needed**

In `crates/flint-viewer/src/panels/render_stats.rs`, add:

```rust
    pub fn fps(&self) -> f32 {
        self.fps
    }
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p flint-viewer`
Expected: Success

- [ ] **Step 7: Commit**

```bash
git add crates/flint-viewer/src/app.rs crates/flint-viewer/src/panels/render_stats.rs
git commit -m "feat(viewer): add F2 rendering stats overlay, replace old FPS panel

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

### Task 6: Visual verification

- [ ] **Step 1: Test in player with a terrain scene**

```bash
cargo run --bin flint -- play demo/rolling_meadow.scene.toml --schemas schemas
```

Press F2 — stats overlay should appear in the top-right corner showing FPS, draw calls, triangles, terrain chunks (drawn/total), grass instances, shadow stats, and resolution. Press F2 again to hide. Press F1 to cycle through debug modes — verify "Wireframe Overlay" appears in the cycle between PBR and Wireframe.

- [ ] **Step 2: Test in viewer**

```bash
cargo run --bin flint -- edit demo/rolling_meadow.scene.toml --schemas schemas
```

Press F2 — same stats overlay should appear. Verify F1 cycles through debug modes including Wireframe Overlay.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 4: Run clippy and fmt**

```bash
cargo fmt --check
cargo clippy -p flint-render -p flint-player -- -D warnings
```

Fix any issues, then commit.
