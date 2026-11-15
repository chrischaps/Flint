# ADR 0074: Runtime meshes have a lifecycle, and every draw is frustum-culled

Status: accepted, 2026-10-12 ([#41](https://github.com/chrischaps/Flint/pull/41)).
Backfilled 2026-11-12 (#374) from commit `7ded8d6`, PR #41's description and
the Vault note "2026-10-12 Capitol Flint City Returns As Runtime Meshes".
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The Capitol builds its city in code, and its terrain arrives over the
network long after the scene loads. A game needed to create, replace and
remove named meshes while running, without leaking GPU memory.

## Decision

- `SceneRenderer::remove_mesh` and `MeshCache::remove`;
  `load_procedural_mesh` is create-or-replace. Draw lists hold `Arc`
  clones, so a replaced or removed mesh's buffers free at the next
  `update_from_world`.
- `ExtensionGpu::upload_mesh` / `remove_mesh` in `pre_update`, and
  `ExtensionCtx::attach_mesh(entity, name)`, which merges
  `{ asset, runtime = true }` into the entity's `model`.
- `model.runtime = true`: the model loader skips meshes that code uploads,
  so runtime-only names no longer log "Model file not found".
- `ImportedMaterial::vertex_colored` and `flint_render::srgb_to_linear`,
  since vertex colours are linear.
- Frustum culling for every game. Each `GpuMesh` gets a local AABB at
  upload; opaque and transparent model draws carry a world AABB (Arvo's
  method) and are culled after the shadow pass, and per cascade in the
  shadow pass. Skinned meshes, wireframes and `bounds` boxes are never
  culled. `RenderStats::culled_draws`; F2 shows `Entities: N (M culled)`.

## Alternatives considered

- A reserved name prefix, or a declare-at-startup call, instead of
  `model.runtime = true`. The flag reads clearly in a scene file, and
  terrain can arrive late.
- Culling only runtime meshes. Culling is on for every static model draw,
  imported glTF included; the bounds come from the vertices, so they can't
  be wrong.

## Consequences

- Culling can't drop a visible draw, but a vertex shader that pushes a
  static model beyond its bounds would pop at screen edges.
- A mesh culls whole, so a large world is built in sectors or nothing
  culls.
- An entity naming a removed mesh draws nothing, or its `bounds` box, until
  a mesh with that name is uploaded again.

## Measured

`flint-render/tests/runtime_mesh.rs` (GPU): 1,000 replacements held
buffers at 26 and DX12 allocator bytes flat. In the Capitol client, 588 MB
held flat over about 1,400 rebuilds of the water.

## Sources

- [#41](https://github.com/chrischaps/Flint/pull/41) / commit
  [`7ded8d6`](https://github.com/chrischaps/Flint/commit/7ded8d6a1ad3577da2296af9931f191eed26a67a)
  "[P1.4] H4: runtime procedural mesh lifecycle with vertex colours and
  bounds (Flint)"
- Vault: "2026-10-12 Capitol Flint City Returns As Runtime Meshes"
- Code now: `crates/flint-render/src/scene_renderer/mod.rs`
  (`remove_mesh`, `cull_draws_to_camera`), `crates/flint-player/src/extension.rs`
  (`ExtensionGpu`, `attach_mesh`); book:
  [Player Extensions](../book/src/guides/player-extensions.md), "Meshes at
  runtime"
