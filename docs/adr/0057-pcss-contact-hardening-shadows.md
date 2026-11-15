# ADR 0057: PCSS on the directional cascades when the sun has a size

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27`, commit `647cc41` and
the code and book lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Part of the Tier 2 renderer pass ([ADR 0054](0054-tier-2-renderer-pass-scope.md)),
listed as "PCSS directional cascades". Directional shadows used a fixed 3x3
PCF kernel, equally soft everywhere. With a source size on the sun
([ADR 0056](0056-source-radius-area-lights.md)) the penumbra could grow with
the distance between occluder and receiver.

## Decision

- When the shadow-casting directional light has a non-zero `angular_size`,
  the cascades use percentage-closer soft shadows: a Vogel-disk blocker
  search finds the average occluder depth, the penumbra width is
  `tan(angular_size)` times the world-space occluder distance, and the
  filter kernel is sized to match, capped at 8 texels.
- A new `vec4` on `ShadowUniforms`, `pcss`: `xyz` is each cascade's
  light-ortho depth range in world units (pull-back plus radius, which turns
  `[0, 1]` depth deltas back into distances), `w` is `tan(angular_size)`.
  `compute_light_matrix` returns that range.
- `w = 0` takes the 3x3 PCF path verbatim, so default or stale uniforms
  render as before.
- The penumbra clamps follow the firefly guards of
  [ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md).
- Mirrored in the shader, skinned, terrain, grass, volumetric and ocean
  WGSL.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Shadows are sharp where an object touches the ground and soften as it
  lifts away. `angular_size = 0` keeps renders byte-identical.
- `ShadowUniforms` grew from 208 to 224 bytes.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Commit
  [`647cc41`](https://github.com/chrischaps/Flint/commit/647cc416c22c96fb667c9b4bb60d9c884d155037)
  "docs: new pages for lighting, music sessions, procgen, debug panels,
  music CLI"
- Code now: `crates/flint-render/src/shadow.rs` (`ShadowUniforms::pcss`,
  `compute_light_matrix`), `crates/flint-render/src/shader.wgsl`,
  `crates/flint-render/src/scene_renderer/render_passes.rs`
- Book: [Lighting](../book/src/concepts/lighting.md), "Shadows"
