# ADR 0105: Rays meet the runtime meshes a game draws, opt-in per mesh

Status: accepted, 2026-10-21 ([#80](https://github.com/chrischaps/Flint/pull/80)).
Backfilled 2026-11-12 (#374) from PR #80, whose body holds the ADR text,
commit `d21a45c`, and the Vault note "2026-10-21 Capitol Camera Meets The
Walls It Draws".

## Context

A game could ask for the ground under the cursor (`screen_to_ground`, ADR
0075; `screen_to_heightfield`, [ADR 0095](0095-ray-against-placed-height-field.md))
but not "where does this line first meet something drawn?". `raycast` meets
physics colliders, and a game that draws its buildings as runtime meshes
([ADR 0074](0074-runtime-meshes-and-frustum-culling.md)) has none. The Capitol's shoulder camera must pull in when a wall
stands between it and the player, so it described every building a second
time, as boxes, kept in step by hand: boxes don't follow roofs or awnings, and
a building that forgets its box gets looked through (TheCapitol#293, H31).

## Decision

A runtime mesh can be a ray target.

- `flint_core::ray_mesh::RayMesh` holds a mesh's triangles under a bounding
  volume hierarchy (median split on the longest centroid axis, at most 4
  triangles a leaf), built from the positions and indices it's drawn with.
  `cast` walks it nearer child first and tests triangles two-sided
  (Möller–Trumbore), so a ray from inside a box meets its far wall.
- `RayScene` places meshes by entity world matrices. `cast`, `cast_filtered`
  and `segment` test each placement's world box, then cast in the mesh's own
  space, and answer a `RayHit`: entity, distance, point, normal (turned to
  face the ray's origin) and triangle index.
- `SceneRenderer::set_ray_mesh(name, Some(mesh))` makes a mesh a target.
  Each `update_from_world` places every entity drawing one in a new
  `RayScene`, before culling, so a target off screen is still met.
  `load_procedural_mesh` and `remove_mesh` end a target.
- `ExtensionGpu::upload_mesh_with(.., MeshOptions { ray_target: true })`
  uploads and builds in one call; `ExtensionCtx::rays` is the scene.
- The player hands the scene to scripts after each update, so a frame's
  scripts cast against what the last frame drew. Scripts:
  `raycast_meshes(ox, oy, oz, dx, dy, dz, max_dist)` and `screen_to_mesh(x,
  y)`, answering with `raycast`'s keys or `()`. Native:
  `Projector::ray_scene()` and `Projector::screen_to_mesh`.

## Alternatives considered

Not recorded in the surviving text. The context names what games had before:
physics colliders through `raycast`, and a hand-kept second description.

## Consequences

- Opt-in: nothing changes for a game that marks no mesh. No new dependencies.
- The triangles are kept on the CPU with the hierarchy. A game that rebuilds
  a big target often pays the build each time, so it should split targets as
  it splits meshes for culling.
- The scene walks its placements one by one: fine for tens to hundreds of
  targets; thousands would want a hierarchy over the placements too.
- One frame behind for anything that moves; static geometry is exact.
- Only runtime meshes can be targets: an imported glTF's CPU geometry isn't
  kept, and skinned meshes would need posed triangles.

## Measured

A build costs about 0.25 µs a triangle in release (200k triangles in 52 ms),
at each upload; a cast about 0.4 µs.

## Sources

- [#80](https://github.com/chrischaps/Flint/pull/80) / commit
  [`d21a45c`](https://github.com/chrischaps/Flint/commit/d21a45cd58a994bc86cba8ec9e9fbea9cc63a788)
  "[H31] Flint: rays meet the meshes a game draws (ADR 0105)"
- Vault: "2026-10-21 Capitol Camera Meets The Walls It Draws"
- Code now: `crates/flint-core/src/ray_mesh.rs`,
  `crates/flint-render/src/scene_renderer/mod.rs` (`set_ray_mesh`,
  `ray_scene`), `crates/flint-player/src/extension.rs`,
  `crates/flint-script/src/projector.rs`, `crates/flint-script/src/api/view.rs`
- Book: [Rendering](../book/src/concepts/rendering.md), "Runtime meshes";
  [Scripting](../book/src/concepts/scripting.md);
  [Player Extensions](../book/src/guides/player-extensions.md)
