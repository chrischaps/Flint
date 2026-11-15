# ADR 0098: Grass stands on the blend of its ground's heights

Status: accepted, 2026-10-20 ([#71](https://github.com/chrischaps/Flint/pull/71)).
Backfilled 2026-11-12 (#374) from PR #71, whose body holds the ADR text,
commit `70aa153`, and the Vault note "The Capitol - grass stands on the blend
(GH-228, ADR 0098)".

## Context

The grass compute pass read a blade's ground height at the nearest texel
(`textureLoad`), because R32Float has no filtering without
`FLOAT32_FILTERABLE`. On a slope every blade in a texel shared one height, so
blades sank into the uphill side of each texel and floated off the downhill
side by up to half a texel's rise. For The Capitol (TheCapitol#228) this
forced a height grid finer than the land's own. Terrain grass also read its
heightmap as cells (`floor(u * N)`), a convention its mesh doesn't use.

## Decision

- Blend four `textureLoad`s round each blade in the compute shader
  (`ground_height`), bilinear, always (not opt-in). No device feature is
  needed.
- A terrain's heightmap is read as its mesh and `sample_height` read it:
  samples on the grid's corners, the first at the origin and the last at the
  far edge.
- A game's `GrassHeight::Map` ([ADR 0089](0089-compute-grass-over-any-ground.md)) is read with each value at the
  middle of its cell, which is what the nearest read implied. Within half a
  cell of the rectangle's edge the height holds level.
- `GrassComputeUniforms::height_at_corners` picks between the two: set for a
  terrain, clear for a game's ground.
- `SceneRenderer::read_grass_blades` reads back the last frame's blades,
  waiting on the GPU, for tests and tools.

## Alternatives considered

Not recorded in the surviving text, beyond the constraint that filtered
sampling of R32Float needs `FLOAT32_FILTERABLE`.

## Consequences

- Blades follow the ground; a ground map can be as coarse as the ground's
  real detail.
- Terrain grass moves by up to a texel's offset to sit on the surface it
  grows from.
- Four loads instead of one per kept cell, not measurable next to the rest
  of the pass.
- A game that sampled its map at cell centres (the documented practice) sees
  only improvement; one that sampled at corners sees a half-cell shift.

## Measured

- Two GPU tests grow grass on a tilted plane, from an 8² ground map and from a
  9² terrain heightmap, and check every blade stands on it to 1 mm. On the
  old nearest read they fail by 0.36 m and 0.7 m.
- In The Capitol's 2048² grid, blended, 99% of blades lie within 2 cm of the
  land (0.15 m read nearest), the worst within 0.22 m (0.41 m).

## Sources

- [#71](https://github.com/chrischaps/Flint/pull/71) / commit
  [`70aa153`](https://github.com/chrischaps/Flint/commit/70aa153dead8bb7d0926d51058443129b665ac27)
  "Grass stands on the blend of its ground's heights (ADR 0098)"
- Vault: "The Capitol - grass stands on the blend (GH-228, ADR 0098) - 2026-10-20"
- Code now: `crates/flint-render/src/grass_compute.wgsl` (`ground_height`),
  `crates/flint-render/src/grass_pipeline.rs` (`height_at_corners`),
  `crates/flint-render/src/scene_renderer/mod.rs` (`read_grass_blades`)
- Book: [Grass on Any Ground](../book/src/concepts/grass-ground.md),
  [Terrain](../book/src/concepts/terrain.md)
