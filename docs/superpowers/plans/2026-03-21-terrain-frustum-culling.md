# Terrain Frustum Culling Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Skip terrain chunks outside the camera frustum to eliminate 50-75% of wasted draw calls.

**Architecture:** Add a `Frustum` struct (6 planes extracted from VP matrix) with an AABB visibility test. Store the camera frustum on `SceneRenderer` each frame. Cull terrain chunks in both the normal and shadow render passes. Store AABB data on each `TerrainDrawCall`. Terrain AABBs are in local space, so the model matrix translation is applied to offset them into world space before testing.

**Tech Stack:** Rust, wgpu 23, flint-render crate

**Spec:** `docs/superpowers/specs/2026-03-21-terrain-frustum-culling-design.md`

---

## Chunk 1: Frustum extraction and AABB test

### Task 1: Create `frustum.rs` with tests

**Files:**
- Create: `crates/flint-render/src/frustum.rs`
- Modify: `crates/flint-render/src/lib.rs:8-63`

- [ ] **Step 1: Create `frustum.rs` with the `Frustum` struct and stub methods**

```rust
/// Camera or light frustum defined by 6 clip planes.
///
/// Each plane `[a, b, c, d]` defines the half-space `ax + by + cz + d >= 0`
/// as visible. Planes are normalized (unit-length normal).
pub struct Frustum {
    pub planes: [[f32; 4]; 6], // left, right, bottom, top, near, far
}

impl Frustum {
    /// Extract 6 frustum planes from a view-projection matrix (Griggs/Hartmann method).
    ///
    /// Works for both perspective and orthographic projections.
    /// The input matrix should be `projection * view` (or `projection * view * model`
    /// if testing AABBs in model-local space).
    pub fn from_view_projection(vp: &[[f32; 4]; 4]) -> Self {
        todo!()
    }

    /// Conservative AABB visibility test (p-vertex method).
    ///
    /// Returns `true` if the AABB is potentially visible (inside or intersecting
    /// the frustum). Returns `false` only when the AABB is fully outside at least
    /// one frustum plane. No false negatives — partially visible boxes always pass.
    pub fn aabb_visible(&self, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> bool {
        todo!()
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/flint-render/src/lib.rs`, add after line 16 (`mod headless;`):

```rust
pub mod frustum;
```

And add after line 62 (`pub use texture_cache::TextureCache;`):

```rust
pub use frustum::Frustum;
```

- [ ] **Step 3: Write failing tests**

Append to `crates/flint-render/src/frustum.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a simple perspective VP looking down -Z from origin.
    /// FOV 90°, aspect 1:1, near 0.1, far 100.
    /// Uses the same [-1,1] NDC convention as the engine's `Camera::perspective_matrix()`.
    fn test_perspective_vp() -> [[f32; 4]; 4] {
        let fov = std::f32::consts::FRAC_PI_2; // 90 degrees
        let aspect = 1.0;
        let near = 0.1_f32;
        let far = 100.0_f32;

        let f = 1.0 / (fov / 2.0).tan();
        let depth = far - near;
        // Perspective projection (right-handed, [-1,1] depth — matches engine convention)
        let proj = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, -(far + near) / depth, -1.0],
            [0.0, 0.0, -(2.0 * far * near) / depth, 0.0],
        ];

        // View: camera at (0,0,0) looking down -Z (identity — already looking down -Z in RH)
        let view: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];

        mat4_mul(&proj, &view)
    }

    /// Helper: multiply two 4×4 matrices (column-major, matching flint_core).
    fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for col in 0..4 {
            for row in 0..4 {
                out[col][row] = a[0][row] * b[col][0]
                    + a[1][row] * b[col][1]
                    + a[2][row] * b[col][2]
                    + a[3][row] * b[col][3];
            }
        }
        out
    }

    #[test]
    fn aabb_in_front_of_camera_is_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Small box at z = -5 (directly in front)
        assert!(frustum.aabb_visible([-1.0, -1.0, -6.0], [1.0, 1.0, -4.0]));
    }

    #[test]
    fn aabb_behind_camera_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box behind camera (positive Z in right-handed looking down -Z)
        assert!(!frustum.aabb_visible([-1.0, -1.0, 5.0], [1.0, 1.0, 10.0]));
    }

    #[test]
    fn aabb_far_left_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box far to the left, at z=-5 but x = -50..-40
        assert!(!frustum.aabb_visible([-50.0, -1.0, -6.0], [-40.0, 1.0, -4.0]));
    }

    #[test]
    fn aabb_partially_intersecting_is_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Large box that straddles the left frustum edge
        assert!(frustum.aabb_visible([-20.0, -1.0, -6.0], [1.0, 1.0, -4.0]));
    }

    #[test]
    fn very_large_aabb_always_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Huge box surrounding everything
        assert!(frustum.aabb_visible(
            [-1000.0, -1000.0, -1000.0],
            [1000.0, 1000.0, 1000.0]
        ));
    }

    #[test]
    fn flat_aabb_on_ground_plane_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Zero-height AABB (flat plane) in front of camera
        assert!(frustum.aabb_visible([-2.0, 0.0, -10.0], [2.0, 0.0, -2.0]));
    }

    #[test]
    fn aabb_beyond_far_plane_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box beyond the far plane (z = -200 to -150, far = 100)
        assert!(!frustum.aabb_visible([-1.0, -1.0, -200.0], [1.0, 1.0, -150.0]));
    }

    #[test]
    fn offset_aabb_simulates_terrain_transform() {
        // Simulates terrain at position [-192, 0, -192]:
        // local AABB [0,0,0]..[96,25,96] → world AABB [-192,0,-192]..[-96,25,-96]
        // Camera looking down -Z should see this (it's at negative Z)
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        assert!(frustum.aabb_visible([-192.0, 0.0, -192.0], [-96.0, 25.0, -96.0]));

        // Local AABB without offset would be at [0,0,0]..[96,25,96] — behind camera
        assert!(!frustum.aabb_visible([0.0, 0.0, 0.0], [96.0, 25.0, 96.0]));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p flint-render frustum -- --nocapture`
Expected: FAIL — `todo!()` panics

- [ ] **Step 5: Implement `from_view_projection`**

Replace the `todo!()` in `from_view_projection`:

```rust
    pub fn from_view_projection(vp: &[[f32; 4]; 4]) -> Self {
        // Columns of the VP matrix (vp is column-major: vp[col][row])
        let c0 = vp[0];
        let c1 = vp[1];
        let c2 = vp[2];
        let c3 = vp[3];

        let mut planes = [[0.0f32; 4]; 6];

        // Left:   row3 + row0  →  c3[row] + c0[row] for each row, but
        // in column-major "row R" means [c0[R], c1[R], c2[R], c3[R]].
        // Plane coefficients: a = c0[R]+c0[R]... no — we combine *rows*:
        //   row_i = [vp[0][i], vp[1][i], vp[2][i], vp[3][i]]
        // Left  = row3 + row0
        planes[0] = [
            c0[3] + c0[0], c1[3] + c1[0], c2[3] + c2[0], c3[3] + c3[0],
        ];
        // Right = row3 - row0
        planes[1] = [
            c0[3] - c0[0], c1[3] - c1[0], c2[3] - c2[0], c3[3] - c3[0],
        ];
        // Bottom = row3 + row1
        planes[2] = [
            c0[3] + c0[1], c1[3] + c1[1], c2[3] + c2[1], c3[3] + c3[1],
        ];
        // Top = row3 - row1
        planes[3] = [
            c0[3] - c0[1], c1[3] - c1[1], c2[3] - c2[1], c3[3] - c3[1],
        ];
        // Near = row3 + row2
        planes[4] = [
            c0[3] + c0[2], c1[3] + c1[2], c2[3] + c2[2], c3[3] + c3[2],
        ];
        // Far = row3 - row2
        planes[5] = [
            c0[3] - c0[2], c1[3] - c1[2], c2[3] - c2[2], c3[3] - c3[2],
        ];

        // Normalize each plane
        for plane in &mut planes {
            let len = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
            if len > 1e-10 {
                plane[0] /= len;
                plane[1] /= len;
                plane[2] /= len;
                plane[3] /= len;
            }
        }

        Self { planes }
    }
```

- [ ] **Step 6: Implement `aabb_visible`**

Replace the `todo!()` in `aabb_visible`:

```rust
    pub fn aabb_visible(&self, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> bool {
        for plane in &self.planes {
            let (a, b, c, d) = (plane[0], plane[1], plane[2], plane[3]);

            // P-vertex: the AABB corner most in the direction of the plane normal
            let px = if a >= 0.0 { aabb_max[0] } else { aabb_min[0] };
            let py = if b >= 0.0 { aabb_max[1] } else { aabb_min[1] };
            let pz = if c >= 0.0 { aabb_max[2] } else { aabb_min[2] };

            // If the p-vertex is behind this plane, the entire AABB is outside
            if a * px + b * py + c * pz + d < 0.0 {
                return false;
            }
        }
        true
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p flint-render frustum -- --nocapture`
Expected: All 8 tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/flint-render/src/frustum.rs crates/flint-render/src/lib.rs
git commit -m "feat(render): add frustum culling with AABB visibility test"
```

## Chunk 2: Wire frustum culling into terrain rendering

### Task 2: Add AABB fields to `TerrainDrawCall`

**Files:**
- Modify: `crates/flint-render/src/terrain_pipeline.rs:17-25`

- [ ] **Step 1: Add world-space `aabb_min` and `aabb_max` fields**

In `crates/flint-render/src/terrain_pipeline.rs`, add two fields to `TerrainDrawCall` (after line 24, `pub model_inv_transpose`):

```rust
    /// World-space AABB minimum corner (chunk local AABB + model translation)
    pub aabb_min: [f32; 3],
    /// World-space AABB maximum corner (chunk local AABB + model translation)
    pub aabb_max: [f32; 3],
```

These store the AABB in **world space** — the chunk's local-space AABB offset by the terrain entity's model matrix translation. This is necessary because terrain entities typically have non-identity transforms (e.g., `position = [-192, 0, -192]`), and the frustum is in world space.

- [ ] **Step 2: Verify it doesn't compile (struct literal errors)**

Run: `cargo check -p flint-render 2>&1 | head -30`
Expected: Errors at all three `TerrainDrawCall { ... }` construction sites missing the new fields.

### Task 3: Supply AABB at all construction sites

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs:602-610` (load_terrain push)
- Modify: `crates/flint-render/src/scene_renderer/mod.rs:679-687` (reload_terrain_geometry push)

Note: `load_terrain_from_data` (line 693) delegates to `reload_terrain_geometry` (line 812), so only two push sites need changing.

- [ ] **Step 1: Update `load_terrain()` push (line 602)**

The chunk AABBs are in local space, but the frustum test operates in world space. The model matrix for terrain is typically a translation (e.g., `position = [-192, 0, -192]`). Extract the translation from the model matrix (`model[3][0..3]`) and offset the AABB.

In `crates/flint-render/src/scene_renderer/mod.rs`, before the `for chunk in chunks` loop (around line 556), add a helper to extract the translation:

```rust
        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];
```

Then change the `self.terrain_draws.push(TerrainDrawCall { ... })` starting at line 602 to include world-space AABBs:

```rust
            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [chunk.aabb_min[0] + tx, chunk.aabb_min[1] + ty, chunk.aabb_min[2] + tz],
                aabb_max: [chunk.aabb_max[0] + tx, chunk.aabb_max[1] + ty, chunk.aabb_max[2] + tz],
            });
```

- [ ] **Step 2: Update `reload_terrain_geometry()` push (line 679)**

Same approach — extract translation from model, offset AABBs. Add after line 632 (`let model_inv_transpose = ...`):

```rust
        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];
```

Then update the push at line 679:

```rust
            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [chunk.aabb_min[0] + tx, chunk.aabb_min[1] + ty, chunk.aabb_min[2] + tz],
                aabb_max: [chunk.aabb_max[0] + tx, chunk.aabb_max[1] + ty, chunk.aabb_max[2] + tz],
            });
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p flint-render`
Expected: Success

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/terrain_pipeline.rs crates/flint-render/src/scene_renderer/mod.rs
git commit -m "feat(render): store AABB on TerrainDrawCall for frustum culling"
```

### Task 4: Store camera frustum on `SceneRenderer` and cull in normal pass

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/mod.rs:110-188` (struct fields)
- Modify: `crates/flint-render/src/scene_renderer/mod.rs:2442` (set frustum in render_to)
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs:705-721` (normal pass terrain loop)

- [ ] **Step 1: Add `camera_frustum` field to `SceneRenderer`**

In `crates/flint-render/src/scene_renderer/mod.rs`, add after the `terrain_material_buffer` field (line 142):

```rust
    camera_frustum: Option<crate::frustum::Frustum>,
```

Find where `SceneRenderer` is constructed (look for `terrain_material_buffer: None`) and add `camera_frustum: None` next to it.

- [ ] **Step 2: Set frustum in `render_to()`**

In `crates/flint-render/src/scene_renderer/mod.rs`, after line 2442 (`let view_proj = camera.view_projection_matrix();`), add:

```rust
        self.camera_frustum = Some(crate::frustum::Frustum::from_view_projection(&view_proj));
```

- [ ] **Step 3: Cull terrain chunks in `render_normal_pass`**

In `crates/flint-render/src/scene_renderer/render_passes.rs`, change the terrain loop (lines 715-721) from:

```rust
                for draw in &self.terrain_draws {
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
```

to:

```rust
                for draw in &self.terrain_draws {
                    if let Some(ref frustum) = self.camera_frustum {
                        if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                            continue;
                        }
                    }
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p flint-render`
Expected: Success

- [ ] **Step 5: Run all tests**

Run: `cargo test -p flint-render`
Expected: All tests pass (frustum unit tests + existing shader parse tests)

- [ ] **Step 6: Commit**

```bash
git add crates/flint-render/src/scene_renderer/mod.rs crates/flint-render/src/scene_renderer/render_passes.rs
git commit -m "feat(render): cull terrain chunks by camera frustum in normal pass"
```

### Task 5: Cull terrain chunks in shadow pass

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs:108-135` (shadow pass terrain loop)

- [ ] **Step 1: Add frustum culling to the shadow pass terrain loop**

In `crates/flint-render/src/scene_renderer/render_passes.rs`, the shadow terrain loop starts at line 108 with `// Render terrain chunks into shadow map`. Change:

```rust
                // Render terrain chunks into shadow map
                for draw in &self.terrain_draws {
```

to:

```rust
                // Render terrain chunks into shadow map (frustum cull per cascade)
                let cascade_frustum = crate::frustum::Frustum::from_view_projection(&cascade_vp);
                for draw in &self.terrain_draws {
                    if !cascade_frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                        continue;
                    }
```

Note: Shadow cascades use orthographic projections — the Griggs/Hartmann extraction handles this correctly. The AABBs stored on `TerrainDrawCall` are already in world space (offset by model translation at load time), so they can be tested directly against the light VP frustum.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p flint-render`
Expected: Success

- [ ] **Step 3: Run all tests**

Run: `cargo test -p flint-render`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/flint-render/src/scene_renderer/render_passes.rs
git commit -m "feat(render): cull terrain chunks by frustum in shadow pass"
```

### Task 6: Visual verification

- [ ] **Step 1: Render a terrain scene to verify no visual regression**

Run against a demo scene with terrain (the Rolling Meadow scene has terrain):

```bash
cargo run --bin flint -- render demo/rolling_meadow.scene.toml --output /tmp/frustum_test.png --schemas schemas --width 1280 --height 720 --distance 30 --pitch 25 --yaw 45
```

Expected: Clean terrain render, identical to before (no missing chunks — camera sees them all from above). Open the PNG and verify terrain is rendered correctly.

- [ ] **Step 2: Render from an angle that should cull some chunks**

```bash
cargo run --bin flint -- render demo/rolling_meadow.scene.toml --output /tmp/frustum_test_close.png --schemas schemas --width 1280 --height 720 --distance 5 --pitch 5 --yaw 0 --target 0,1,0
```

Expected: Scene renders correctly from close up. Chunks behind the camera are culled but this is invisible — the visible terrain should look correct.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass across the workspace

- [ ] **Step 4: Final commit (if any formatting/clippy fixes needed)**

```bash
cargo fmt --check
cargo clippy -p flint-render -- -D warnings
```

Fix any issues, then commit.
