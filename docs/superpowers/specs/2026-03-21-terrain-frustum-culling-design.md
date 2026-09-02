# Terrain Frustum Culling

**Date:** 2026-03-21
**Status:** Approved
**Scope:** `flint-render` only — no changes to `flint-terrain` or other crates

## Problem

All terrain chunks are submitted to the GPU every frame regardless of camera visibility. On a 4×4 chunk grid, a typical camera view only sees 25-50% of chunks, so 50-75% of draw calls are wasted. This affects both the normal pass and shadow passes.

## Design

### New file: `crates/flint-render/src/frustum.rs`

**`Frustum` struct** — 6 clip planes extracted from a view-projection matrix:

```rust
pub struct Frustum {
    pub planes: [[f32; 4]; 6], // left, right, bottom, top, near, far
}
```

Each plane is `[a, b, c, d]` where `ax + by + cz + d >= 0` is the visible half-space. Planes are normalized (unit-length normal).

**`Frustum::from_view_projection(vp: &[[f32; 4]; 4]) -> Frustum`** — Griggs/Hartmann extraction method. For each plane pair, add or subtract rows of the VP matrix:

- Left: row3 + row0
- Right: row3 - row0
- Bottom: row3 + row1
- Top: row3 - row1
- Near: row3 + row2
- Far: row3 - row2

Normalize each plane by dividing by the length of `[a, b, c]`.

**`Frustum::aabb_visible(&self, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> bool`** — p-vertex test. For each plane, compute the "positive vertex" (the AABB corner most in the direction of the plane normal). If that vertex is behind the plane, the entire AABB is outside — return false. If all 6 planes pass, return true.

The p-vertex for plane normal `[a, b, c]`:
- x = if a >= 0 { aabb_max[0] } else { aabb_min[0] }
- y = if b >= 0 { aabb_max[1] } else { aabb_min[1] }
- z = if c >= 0 { aabb_max[2] } else { aabb_min[2] }

This is conservative: partially-visible chunks are always drawn (no false negatives).

### Changes to `TerrainDrawCall`

In `crates/flint-render/src/terrain_pipeline.rs`, add AABB fields:

```rust
pub struct TerrainDrawCall {
    // ... existing fields ...
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}
```

### Changes to `TerrainDrawCall` construction sites

All sites that construct `TerrainDrawCall` must supply `aabb_min` and `aabb_max`. There are three:

1. `load_terrain()` (~line 602 of `mod.rs`) — primary load path
2. `reload_terrain_geometry()` (~line 679 of `mod.rs`) — editor brush sculpting path
3. `load_terrain_from_data()` (~line 812 of `mod.rs`) — delegates to `reload_terrain_geometry()`

Each copies the AABB from the chunk:

```rust
self.terrain_draws.push(TerrainDrawCall {
    // ... existing fields ...
    aabb_min: chunk.aabb_min,
    aabb_max: chunk.aabb_max,
});
```

### Changes to normal pass

In `render_passes.rs`, the terrain loop becomes:

```rust
let frustum = Frustum::from_view_projection(&view_proj);
for draw in &self.terrain_draws {
    if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
        continue;
    }
    // ... existing draw call code ...
}
```

The `view_proj` matrix is computed in `render_to()` but not currently available inside `render_normal_pass()`. Store the camera frustum as a field on `SceneRenderer` — set it during `update_per_frame_uniforms()` or at the top of `render_to()`:

```rust
self.camera_frustum = Some(Frustum::from_view_projection(&view_proj));
```

Then `render_normal_pass()` accesses `self.camera_frustum` without any signature changes.

**Model transform note:** The frustum test uses `view_proj * model` as the combined matrix when extracting planes (or equivalently, transforms the AABB into world space). In practice terrain model matrices are identity or simple translations, but the implementation should handle the general case by incorporating `draw.model` into the VP before extraction, or by transforming the AABB min/max by the model matrix before testing.

### Changes to shadow pass

For each shadow cascade, extract a frustum from that cascade's light view-projection matrix and cull terrain chunks against it. The cascade VP is already available in the shadow pass loop.

### Module registration

Add `mod frustum;` and `pub use frustum::Frustum;` in `crates/flint-render/src/lib.rs`.

## Testing

Unit tests in `frustum.rs`:

1. **Known frustum** — construct a perspective VP looking down -Z, verify:
   - AABB in front of camera → visible
   - AABB behind camera → culled
   - AABB far to the left → culled
   - AABB partially intersecting frustum edge → visible (conservative)
2. **Degenerate cases** — zero-volume AABB (flat plane), very large AABB (always visible)

## Files Changed

| File | Change |
|------|--------|
| `crates/flint-render/src/frustum.rs` | **New** — `Frustum` struct, extraction, AABB test |
| `crates/flint-render/src/lib.rs` | Add `mod frustum` |
| `crates/flint-render/src/terrain_pipeline.rs` | Add `aabb_min`/`aabb_max` to `TerrainDrawCall` |
| `crates/flint-render/src/scene_renderer/mod.rs` | Copy AABB in `load_terrain()`, `reload_terrain_geometry()`, `load_terrain_from_data()` |
| `crates/flint-render/src/scene_renderer/render_passes.rs` | Frustum cull in normal + shadow terrain loops |

## Notes

- **Projection agnostic:** The Griggs/Hartmann extraction works for both perspective and orthographic projection matrices. No special-casing needed.
- **Grass rendering unaffected:** Grass uses a single GPU-instanced draw call covering the entire terrain, not per-chunk draws. It is not culled by this change.

## Out of Scope

- Mesh entity frustum culling (future work, can reuse `Frustum`)
- Occlusion culling
- Temporal coherence / caching visibility flags across frames
- LOD selection (Priority 2 on terrain roadmap)
- Grass instance culling (single instanced draw, not per-chunk)
