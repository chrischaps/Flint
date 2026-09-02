# Rendering Stats Overlay

**Date:** 2026-03-22
**Status:** Approved
**Scope:** `flint-render` (stats collection), `flint-player` (integration + overlay rendering), `flint-viewer` (integration + overlay rendering, replaces old panel)

## Problem

There is no rendering statistics overlay in the player, and the viewer's existing stats panel only shows FPS and frame time. Developers need Unity-style rendering statistics (draw calls, triangles, culling info, per-system breakdowns) to diagnose performance issues and verify optimizations like terrain frustum culling.

## Design

### New file: `crates/flint-render/src/render_stats.rs`

**`RenderStats` struct** — all rendering metrics for a single frame:

```rust
pub struct RenderStats {
    // Timing (set by caller — player/viewer have access to frame timing)
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
    pub transparent_draws: u32,      // includes transparent_skinned_draws
    pub billboard_draws: u32,
    pub particle_draws: u32,
    pub particle_instances: u32,
    pub sprite_batches: u32,
    pub grass_instances: u32,
    pub grass_draw_calls: u32,       // 0 or 1
    // Shadow pass
    pub shadow_draw_calls: u32,
    pub shadow_triangles: u32,
    // Screen info
    pub resolution: [u32; 2],
}
```

Implements `Default` for initialization.

**`format_count(n: u32) -> String`** — human-readable formatting helper. Lives in this file alongside the struct. Values >= 1,000,000 display as `1.2M`, >= 1,000 as `1.2K`, otherwise plain number.

Note: `flint-render` does NOT depend on `egui`, and we keep it that way. This file only contains the `RenderStats` data struct and the formatting helper — no UI code. The overlay rendering function lives in the consumer crates (player and viewer), which already depend on `egui`.

### Overlay rendering (in player and viewer)

**`render_stats_overlay(ctx: &egui::Context, stats: &RenderStats)`** — a small function duplicated in both `flint-player` and `flint-viewer` (or extracted into a shared utility if duplication becomes a problem). Renders the stats as a semi-transparent egui `Window` anchored to the top-right corner. Layout:

```
┌─ RENDERING STATS ──────────┐
│ FPS: 144.0 (6.9ms)         │
│ Draw Calls: 87              │
│ Triangles: 142.3K           │
│ ─────────────────────────── │
│ Entities: 34                │
│ Skinned: 4                  │
│ Terrain: 12/64 chunks       │
│ Transparent: 8              │
│ Billboards: 6               │
│ Particles: 3 (2,400 inst)   │
│ Sprites: 2                  │
│ Grass: 48,000 inst          │
│ ─────────────────────────── │
│ Shadow Calls: 52            │
│ Shadow Tris: 89.1K          │
│ ─────────────────────────── │
│ 1280×720                    │
└─────────────────────────────┘
```

Style: dark semi-transparent background (`rgba(0,0,0,0.82)`), monospace font, FPS in green. The window is non-interactive (no title bar drag, no resize, no close button).

Numbers use human-readable formatting: values >= 1000 display as `1.2K`, >= 1000000 as `1.2M`.

### Changes to `SceneRenderer`

**`pub fn collect_stats(&self) -> RenderStats`** — aggregates stats from internal draw call vectors:

- `draw_calls`: sum of all draw call vector lengths (entity_draws + skinned_entity_draws + terrain_draws + transparent_draws + transparent_skinned_draws + billboard_draws + particle_draws + sprite2d_batches + grass (0 or 1))
- `triangles`: sum per type, since not all draw call structs have `index_count`:
  - `entity_draws`, `skinned_entity_draws`, `transparent_draws`, `transparent_skinned_draws`: each has `index_count` field → sum `index_count / 3`
  - `terrain_draws`: each has `index_count` → sum `index_count / 3`
  - `billboard_draws`: no `index_count` — each billboard is a fixed quad → 2 triangles per draw
  - `particle_draws`: no `index_count` — each uses instanced quads → `instance_count * 2` per draw
  - `sprite2d_batches`: no `index_count` — each instance is a quad → `instance_count * 2` per batch
  - grass: uses `BLADE_INDEX_COUNT` indices per instance → `grass_instance_count * BLADE_INDEX_COUNT / 3`
- Per-system counts: `.len()` of each vector
- `transparent_draws`: includes both `transparent_draws.len()` and `transparent_skinned_draws.len()`
- `terrain_total_chunks`: total chunks before culling. Store `terrain_total_chunks: u32` on `SceneRenderer`, set during `load_terrain()` and `load_terrain_from_data()` from the chunks slice length.
- `grass_instances`: from `self.grass_instance_count`
- `grass_draw_calls`: 1 if `grass_instance_count > 0`, else 0
- `shadow_draw_calls` and `shadow_triangles`: estimated from main pass data × `CASCADE_COUNT` (currently 3, defined in `shadow.rs`). This overestimates (doesn't account for shadow frustum culling) but gives useful order-of-magnitude.
- `fps` and `frame_time_ms`: set to 0.0 by `collect_stats()` — the caller fills these in from their own timing source.
- `resolution`: set to `[0, 0]` — the caller fills this in from their render context.

### Player integration

In `crates/flint-player/src/player_app/mod.rs`:

- Add `show_stats: bool` field (default false) and `stats_fps_samples: VecDeque<f64>` for FPS smoothing
- F2 key handler: toggle `show_stats`
- After `render_to()`, if `show_stats`: call `self.scene_renderer.collect_stats()`, fill in FPS (from smoothed `GameClock.delta_time`), frame_time_ms, and resolution, then call `render_stats_overlay(&self.egui_ctx, &stats)` during the egui pass
- FPS smoothing: maintain a rolling window of delta_time samples over the last ~0.5s, display the average

### Free up F2 in the viewer

The viewer currently uses F2 for wireframe overlay (shaded geometry + wireframe lines on top). This is distinct from `WireframeOnly` in the F1 debug mode cycle, which shows only wireframe lines. To free F2 for the stats overlay, fold wireframe overlay into the F1 cycle as a new `DebugMode::WireframeOverlay` variant:

**Changes to `crates/flint-render/src/debug.rs`:**
- Add `WireframeOverlay` variant to `DebugMode` enum (between `Pbr` and `WireframeOnly`)
- Update `next()` cycle: `Pbr → WireframeOverlay → WireframeOnly → Normals → Depth → UV → Unlit → MetalRough → Pbr`
- `WireframeOverlay` returns `as_u32() = 0` (same as `WireframeOnly` — shader stays PBR, wireframe handled by pipeline swap)
- Update `label()` to return `"Wireframe Overlay"` for the new variant

**Changes to render passes:**
- In `render_normal_pass` and the wireframe overlay section, check for `self.debug_state.mode == DebugMode::WireframeOverlay` instead of `self.debug_state.wireframe_overlay`
- The `wireframe_overlay: bool` field on `DebugState` can be removed (replaced by the enum variant)

**Changes to viewer (`crates/flint-viewer/src/app.rs`):**
- Remove the F2 wireframe overlay handler
- Remove the F3 normal arrows handler (already accessible via F1 cycling to Normals mode, or keep F3 as-is if normal arrows are a separate overlay concept)

This frees F2 for the stats overlay in both the player and viewer.

### Viewer integration

In `crates/flint-viewer/src/app.rs`:

- Replace the existing `RenderStats` usage with the new shared `collect_stats()` + `render_stats_overlay()`
- Add `show_stats: bool` field (default false)
- F2 key handler: toggle `show_stats` (now free after wireframe overlay moved to F1 cycle)
- The viewer must retain its own FPS tracking mechanism (rolling `VecDeque<Instant>` window, same pattern as existing `RenderStats`) to populate `fps`/`frame_time_ms` on the struct before passing to the overlay function.
- Remove or simplify `crates/flint-viewer/src/panels/render_stats.rs` (the old FPS-only panel)

### Module registration

Add `pub mod render_stats;` and `pub use render_stats::{RenderStats, format_count};` in `crates/flint-render/src/lib.rs`. No new dependencies — `RenderStats` is a plain data struct.

## Shadow Pass Stats — Simplified Approach

Computing exact shadow pass draw calls is complex (per-cascade frustum culling, conditional grass rendering, etc.). Instead of tracking these at render time, estimate from the main pass data:

- `shadow_draw_calls`: (entity_draws + skinned_draws + terrain_draws_visible) * CASCADE_COUNT (currently 3, from `shadow.rs`)
- `shadow_triangles`: corresponding triangle sum * CASCADE_COUNT

This is an overestimate (doesn't account for shadow frustum culling) but gives a useful order-of-magnitude. If precise shadow stats are needed later, the shadow pass can be instrumented to count actual submissions.

## Testing

Unit tests in `render_stats.rs`:

1. **Number formatting** — verify `format_count()` produces correct K/M suffixes
2. **Default stats** — verify `RenderStats::default()` has all zeros

The overlay rendering itself is visual and tested via `flint render` / `flint play` visual verification.

## Files Changed

| File | Change |
|------|--------|
| `crates/flint-render/src/render_stats.rs` | **New** — `RenderStats` struct, `format_count()` helper |
| `crates/flint-render/src/lib.rs` | Add `mod render_stats`, pub use exports |
| `crates/flint-render/src/debug.rs` | Add `WireframeOverlay` variant to `DebugMode`, update `next()` cycle, remove `wireframe_overlay` bool from `DebugState` |
| `crates/flint-render/src/scene_renderer/mod.rs` | Add `terrain_total_chunks: u32` field (set in `load_terrain()` and `load_terrain_from_data()`), add `pub fn collect_stats(&self) -> RenderStats`, update wireframe overlay references |
| `crates/flint-render/src/scene_renderer/render_passes.rs` | Check `DebugMode::WireframeOverlay` instead of `wireframe_overlay` bool |
| `crates/flint-player/src/player_app/mod.rs` | Add `show_stats` bool, F2 handler, `render_stats_overlay()` function, stats collection + overlay rendering in frame loop |
| `crates/flint-viewer/src/app.rs` | Add `show_stats` bool, F2 handler, `render_stats_overlay()` function, replace old stats with new overlay, remove old F2/F3 wireframe/normal handlers |
| `crates/flint-viewer/src/panels/render_stats.rs` | Remove or simplify (replaced by shared impl) |

## Out of Scope

- GPU memory tracking
- Per-system CPU timing breakdown
- Frame time graph / history
- Detailed per-material or per-shader stats
- Stats logging to file
