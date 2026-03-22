# Rendering Stats Overlay

**Date:** 2026-03-22
**Status:** Approved
**Scope:** `flint-render` (stats collection + overlay), `flint-player` (integration), `flint-viewer` (integration, replaces old panel)

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
    pub transparent_draws: u32,
    pub billboard_draws: u32,
    pub particle_draws: u32,
    pub particle_instances: u32,
    pub sprite_batches: u32,
    pub grass_instances: u32,
    // Shadow pass
    pub shadow_draw_calls: u32,
    pub shadow_triangles: u32,
    // Screen info
    pub resolution: [u32; 2],
}
```

Implements `Default` for initialization.

**`render_stats_overlay(ctx: &egui::Context, stats: &RenderStats)`** — renders the stats as a semi-transparent egui `Window` anchored to the top-right corner. Layout:

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

- `draw_calls`: sum of all draw call vector lengths (entity_draws + skinned_entity_draws + terrain_draws + transparent_draws + transparent_skinned_draws + billboard_draws + particle_draws + sprite2d_batches)
- `triangles`: sum of all `index_count / 3` across all draw call types. For particles, each particle draw uses instanced rendering so triangle count = instance_count * particle_mesh_tris. For sprites, each instance = 2 triangles (quad).
- Per-system counts: `.len()` of each vector
- `terrain_total_chunks`: `terrain_draws.len()` reflects drawn chunks. Total chunks requires knowing the original chunk count — store `terrain_total_chunks: u32` on `SceneRenderer`, set during `load_terrain()` from the chunks slice length.
- `grass_instances`: from `self.grass_instance_count`
- `shadow_draw_calls` and `shadow_triangles`: these are harder to collect since the shadow pass creates temporary buffers. Instead, compute them from the same draw call vectors (same entity/terrain draws are submitted per cascade, minus culled chunks). For simplicity, report the count of drawable entities + terrain chunks * CASCADE_COUNT as an estimate. Or omit and just report the main pass stats.
- `fps` and `frame_time_ms`: set to 0.0 by `collect_stats()` — the caller fills these in from their own timing source.
- `resolution`: set to `[0, 0]` — the caller fills this in from their render context.

### Player integration

In `crates/flint-player/src/player_app/mod.rs`:

- Add `show_stats: bool` field (default false) and `stats_fps_samples: VecDeque<f64>` for FPS smoothing
- F2 key handler: toggle `show_stats`
- After `render_to()`, if `show_stats`: call `self.scene_renderer.collect_stats()`, fill in FPS (from smoothed `GameClock.delta_time`), frame_time_ms, and resolution, then call `render_stats_overlay(&self.egui_ctx, &stats)` during the egui pass
- FPS smoothing: maintain a rolling window of delta_time samples over the last ~0.5s, display the average

### Viewer integration

In `crates/flint-viewer/src/app.rs`:

- Replace the existing `RenderStats` usage with the new shared `collect_stats()` + `render_stats_overlay()`
- Add `show_stats: bool` field (default false)
- F2 key handler: toggle `show_stats`
- Remove or simplify `crates/flint-viewer/src/panels/render_stats.rs` (the old FPS-only panel)

### Module registration

Add `pub mod render_stats;` and `pub use render_stats::{RenderStats, render_stats_overlay};` in `crates/flint-render/src/lib.rs`.

Note: `flint-render` already depends on `egui` (used by the viewer for inspector UI), so adding egui rendering code here requires no new dependencies.

## Shadow Pass Stats — Simplified Approach

Computing exact shadow pass draw calls is complex (per-cascade frustum culling, conditional grass rendering, etc.). Instead of tracking these at render time, estimate from the main pass data:

- `shadow_draw_calls`: (entity_draws + skinned_draws + terrain_draws_visible) * CASCADE_COUNT (currently 4)
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
| `crates/flint-render/src/render_stats.rs` | **New** — `RenderStats` struct, `collect_stats()` return type, `render_stats_overlay()` egui function, number formatting |
| `crates/flint-render/src/lib.rs` | Add `mod render_stats`, pub use exports |
| `crates/flint-render/src/scene_renderer/mod.rs` | Add `terrain_total_chunks: u32` field, set in `load_terrain()`, add `pub fn collect_stats(&self) -> RenderStats` |
| `crates/flint-player/src/player_app/mod.rs` | Add `show_stats` bool, F2 handler, stats collection + overlay rendering in frame loop |
| `crates/flint-viewer/src/app.rs` | Add `show_stats` bool, F2 handler, replace old stats with new overlay |
| `crates/flint-viewer/src/panels/render_stats.rs` | Remove or simplify (replaced by shared impl) |

## Out of Scope

- GPU memory tracking
- Per-system CPU timing breakdown
- Frame time graph / history
- Detailed per-material or per-shader stats
- Stats logging to file
