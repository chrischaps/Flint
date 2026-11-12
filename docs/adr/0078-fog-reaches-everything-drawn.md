# ADR 0078: A pixel is sky only at depth 1.0, so fog reaches everything drawn

Status: accepted, 2026-10-16 ([#45](https://github.com/chrischaps/Flint/pull/45)).
Backfilled 2026-11-12 (#374) from commit `919d651` and PR #45's
description. No ADR text separate from the commit and PR survives.

## Context

The composite fog skipped any pixel with depth ≥ 0.9999 to spare the sky.
With near = 1 m that cut is at about `1 / (0.0001 + 1/far)` m: 2.9 km at
far = 4 km, and never past 10 km whatever `far` is. Geometry beyond it was
drawn unfogged, so ground running to the horizon ended in a dark band under
the sky. SSAO, volumetrics and depth of field used the same cut.

To hide that, the ocean fogged itself over a fixed 380-640 m of view depth,
at full strength whenever `fog_enabled` was on. It ignored `fog_density`
and `fog_start`, so a scene that thinned its fog still had fog-coloured
water 400-600 m out.

## Decision

- The sky and skybox never write depth, and every scene pass clears it to
  exactly 1.0. So a pixel is sky only when its depth is 1.0. Composite fog,
  DoF, the Tron and underwater modes, SSAO and volumetrics all test
  `depth >= 1.0`. Anything drawn inside the far plane is < 1.0
  (Depth32Float keeps about 6e-8 of precision there), so it's fogged however
  far away it is.
- The ocean's self-fog is gone. The composite fogs the water like any other
  surface, with the scene's own density and start. `OceanUniforms` loses
  `fog_color` (832 to 816 bytes).

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- No new API and nothing to opt into. Scenes whose geometry all lies within
  about 3 km look the same. Geometry further out is fogged, as the scene's
  fog settings already asked. With thin fog, water keeps its colour further
  out.
- The book says to set `far` to what the scene draws, not larger.
- Still holds: `is_sky_depth` in `composite_shader.wgsl` is the shared
  test.

## Sources

- [#45](https://github.com/chrischaps/Flint/pull/45) / commit
  [`919d651`](https://github.com/chrischaps/Flint/commit/919d651b15ca787d67f365525a3a5ead64b7e1e6)
  "[H10] Fog reaches everything drawn, at every zoom (Flint)"
- Vault: "The Capitol - Fog Reaches Everything (H10) - 2026-10-16"
- Code now: `crates/flint-render/src/composite_shader.wgsl`
  (`is_sky_depth`), `crates/flint-render/src/ocean_shader.wgsl`; book:
  [Post-Processing](../book/src/concepts/post-processing.md),
  [Ocean](../book/src/concepts/ocean.md)
