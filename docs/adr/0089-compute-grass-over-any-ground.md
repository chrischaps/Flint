# ADR 0089: Compute grass over any ground

Status: accepted, 2026-10-17 ([#59](https://github.com/chrischaps/Flint/pull/59)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #59's description,
written there as "[ADR 0088](0088-shoreline-foam-from-scene-depth.md)", and commit `2754576`. #58 (shoreline foam)
merged first as 0088, and PR #60 renumbered this one to 0089.
Amended by [ADR 0092](0092-grass-culled-to-view-drawn-indirect.md) (view culling and an indirect draw), [ADR 0093](0093-repaint-grass-mask-in-place.md) (repaint
part of the mask) and [ADR 0098](0098-grass-stands-on-blended-heights.md) (blended ground heights).

## Context

Flint's compute grass read a terrain's heightmap for height and one splat
layer for density. Its compute pass walked the whole terrain's grid every
frame, with an instance buffer sized at half the terrain's area times the
density. Games whose ground isn't a Flint terrain (runtime meshes, box
levels, spline tracks) couldn't use it. At city or region scale the area
sizing doesn't fit in memory: 5.7 GB for 3 km at density 40. The Capitol's
ground is a runtime mesh several kilometres across.

## Decision

1. A game may give the grass pass its own ground: a world rectangle, an R8
   density mask stretched over it, and a height (a flat `y`, or an R32F
   heightmap of world y over the same rectangle), through
   `SceneRenderer::set_grass_ground` / `ExtensionGpu::set_grass_ground`, or
   a `grass_ground` scene component for flat ground. It reuses the terrain
   path's bind group layout. The mask binds where the splat map does,
   channel 0, and the heights bind as the heightmap with offset 0 and scale
   1, so the shaders don't fork. One grass field per renderer; the last set
   wins.
2. The mask can thin the field in proportion (`scale_by_mask`, default on
   for ground): a blade over the threshold is kept with probability equal
   to the weight. The terrain path keeps its threshold-only rule.
3. The compute dispatch covers only the cells within `max_distance` of the
   camera (an axis-aligned square, ±1 cell for jitter), clipped to the
   ground. Cells are indexed in the whole grid, so placement is identical
   to a full dispatch. The buffer holds the smaller of the old area bound
   and the reach disc +10%.
4. Grass owns its view uniform bind group instead of borrowing a terrain
   chunk's.

## Alternatives considered

- A per-mesh "grass" material flag that scatters on triangles: general,
  but needs triangle-area sampling and a per-mesh buffer, and the Capitol's
  ground is many meshes.
- A Rhai-paintable mask: later, on top of this API.
- Keeping the full dispatch and sizing the buffer to the reach only: still
  walks millions of empty cells per frame at region scale.

## Consequences

- Mesh-ground games get grass with no terrain. Cost follows the grass
  within reach, not the ground's size. Terrain grass is unchanged in look,
  and cheaper on big terrains.
- Scene transitions clear the grass; an old scene's grass used to stay.
- A shaped heightmap is Rust-only; the `grass_ground` component is level
  ground.
- Since then: the record left out-of-view blades drawn and a blocking count
  read-back; [ADR 0092](0092-grass-culled-to-view-drawn-indirect.md) culls blades to the view and draws indirect with no
  read-back. It said a mask is uploaded whole on each change; [ADR 0093](0093-repaint-grass-mask-in-place.md) adds
  `update_grass_mask` for part of it. [ADR 0098](0098-grass-stands-on-blended-heights.md) stands a blade on the blend
  of its ground's heights.
- Since then: [#825](https://github.com/chrischaps/Flint/pull/825) moved the
  `grass_ground` loader into `flint-render`
  (`load_scene_grass_ground`), so `flint edit` draws it too.

## Measured

RTX 4070, `flint-player --exit-after 10`, `FLINT_VSYNC=0`, alternating runs:

| Scene | ms/frame | Triangles |
|---|---|---|
| Mesh Meadow, grass off | 2.37 / 2.54 | 25k |
| Mesh Meadow, grass on (density 40, 70 m, ~110k blades) | 3.96 / 3.79 | 1.34 M |
| Same mask stretched to 3 km, in an unbroken field (~440k blades) | 6.05 / 5.98 | 5.3 M |

A 3 km ground at density 40 takes 21 MB of instances instead of 5.7 GB.

## Sources

- [#59](https://github.com/chrischaps/Flint/pull/59) / commit
  [`2754576`](https://github.com/chrischaps/Flint/commit/275457626143b84069f1ca141191c8f617813505)
  "[H21] Compute grass over any ground: a density mask and a height
  (Flint)"
- [#60](https://github.com/chrischaps/Flint/pull/60) / commit
  [`0dea79f`](https://github.com/chrischaps/Flint/commit/0dea79f6caa3c31457a8d368c09087cbb15ecc84)
  "[H21] Grass over any ground is ADR 0089 (H22 took 0088 first)"
- [#825](https://github.com/chrischaps/Flint/pull/825) / commit
  [`04ae154`](https://github.com/chrischaps/Flint/commit/04ae15427496eaeac380afe51b4e1541ea80ad15)
  "flint edit draws a scene's grass_ground, through the player's own
  loader"
- Vault: "The Capitol - grass over any ground (H21, GH-124) - 2026-10-17"
- Code now: `crates/flint-render/src/grass_pipeline.rs` (`GrassGround`),
  `crates/flint-render/src/scene_renderer/grass_ground.rs`,
  `crates/flint-render/src/grass_compute.wgsl`,
  `schemas/components/grass_ground.toml`, `examples/mesh_meadow.scene.toml`;
  book: [Grass on Any Ground](../book/src/concepts/grass-ground.md)
