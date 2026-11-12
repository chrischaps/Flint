# ADR 0088: Shoreline foam from scene depth

Status: accepted, 2026-10-17 ([#58](https://github.com/chrischaps/Flint/pull/58)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #58's description and
commit `0edc974`.
Amended by [ADR 0091](0091-shore-foam-measured-across-the-water.md) (a screen-space shore search measures the band across
the water).

## Context

The ocean foams at wave crests, round an `ocean_contact` hull and in the
rain. Nothing foamed where the water meets a shore, so harbour walls, piers
and canal banks ended in a clean, hard edge. Masks (a painted shore
texture, or a distance field the game supplies) are accurate, but every
game would have to make one.

## Decision

Derive the shore from the depth buffer the ocean already reads for
refraction.

- Depth below the surface, not along the ray. The two view-z depths on one
  ray scale the camera→surface vector out to the scene point, and the
  measure is the surface's height minus that point's. A beach then gives a
  band of the same width from every angle, and the waves move it for free.
  Measured along the ray, it would widen as the camera flattened.
- The undistorted pixel. Refraction's offset would make the band shimmer.
- Opt-in. `shore_foam_width = 0` is the default, so no existing scene
  changes. It's forced off without the grab pass (no post-processing).
- Its own colour, because shore foam is often warmer or dirtier than crest
  foam. It's still lit like the crest foam.

The `ocean` component gains `shore_foam_width`, `shore_foam_strength`,
`shore_foam_speed` and `shore_foam_color`; `OceanUniformsGpu` grows from 816
to 848 bytes. The F3 ocean panel gains a Shoreline Foam section.

## Alternatives considered

- A mask: a painted shore texture, or a distance field the game supplies.
  Accurate, but every game would have to make one.

## Consequences

- No mask, and any shoreline works: walls, piers, rocks, beaches.
- A vertical wall only foams on the face the camera sees. At the foot of
  the near wall, the ray looks past the wall to the bed.
- One extra depth load and an fbm per water pixel when enabled. Nothing
  when disabled (a uniform branch).
- Since then: on a wall this measure made the band
  `shore_foam_width / tan(elevation)` wide on the water, broad from a low
  camera and a hairline from overhead. [ADR 0091](0091-shore-foam-measured-across-the-water.md) (#63) added
  `shore_foam_reach`, a screen-space search for the shore that measures
  the band across the water, so a wall's band keeps its width from any
  angle and the near wall foams from its kerb. With reach 0, the default,
  the band is this ADR's depth test. Water bodies share the foam
  (`water_body.rs`, ADR 0088/0091).

## Sources

- [#58](https://github.com/chrischaps/Flint/pull/58) / commit
  [`0edc974`](https://github.com/chrischaps/Flint/commit/0edc9744cc6184c15316f7accfaf0ff72decba8a)
  "Shoreline foam on the ocean, from scene depth (ADR 0088)"
- Not this decision: [#59](https://github.com/chrischaps/Flint/pull/59)
  (grass over any ground) also took 0088 and was renumbered to
  [ADR 0089](0089-compute-grass-over-any-ground.md) by #60.
- Vault: "The Capitol - shoreline foam from scene depth (H22, GH-126) -
  2026-10-17"
- Code now: `crates/flint-render/src/ocean_shader.wgsl`,
  `crates/flint-render/src/ocean_pipeline.rs` (`shore_foam_*`),
  `crates/flint-render/src/water_body.rs`; book:
  [Ocean](../book/src/concepts/ocean.md), "Shoreline foam"
