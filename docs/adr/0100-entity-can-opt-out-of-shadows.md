# ADR 0100: An entity can opt out of casting shadows

Status: accepted, 2026-10-20 ([#73](https://github.com/chrischaps/Flint/pull/73)).
Backfilled 2026-11-12 (#374) from PR #73, whose body holds the ADR text,
commit `4877c38`, and the Vault note "The Capitol - mist casts no shadow
(GH-232, ADR 0100)".

## Context

The shadow pass is depth-only and doesn't know about opacity, alpha or blend
mode, so anything drawn casts fully: a sheet at 2% opacity cast a shadow as
dark as a wall. That's right for most solid things and wrong for mist, glass,
spray, smoke cards and other translucent stand-ins. The only lever was the
global shadow switch. The Capitol's valley mist, translucent sheets 0.5 to
2.6 m over the ground, shaded the ground at noon when their opacity was 0
(TheCapitol#232).

## Decision

One opt-out per entity, on the material: `material.cast_shadows` (bool,
default true), inherited from the parent like the other per-frame material
fields.

- Read each frame in extraction into a `casts_shadow` flag on `DrawCall` and
  `SkinnedDrawCall`. Both of `render_shadow_pass`'s loops (the uniform gather
  and the draws) test it, so their slots line up. Toggling it needs no
  material rebuild.
- Covers opaque, transparent and skinned draws, and imported, runtime and
  procedural (`bounds`) meshes. Terrain chunks and grass don't read it.
- The F2 shadow stats count only the draws that cast.

## Alternatives considered

- **Automatic by opacity.** Rejected: a nearly clear sheet can still be meant
  to cast (a stained window), and a threshold would make a fading entity's
  shadow pop.
- **Alpha-testing in the shadow pass** (shadows that follow a texture's
  cut-outs). Kept separate and later; it needs the shadow pipeline to sample
  the material.

## Consequences

- Games can turn shadows off wherever they're wrong at no cost; every
  existing scene is as it was.
- A draw casts fully or not at all: partial, coloured or opacity-weighted
  shadows still aren't possible.
- The Capitol later used it to keep about 100 flat land chunks out of the
  low sun's cascades, which had cost about 0.7 ms (Vault, 2026-10-21).

## Measured

`cast_shadows_false_keeps_a_mesh_out_of_the_shadow_map`: at 2% opacity the
sheet shadows the ground (0 against 194); with `cast_shadows = false` the
ground matches the unshadowed frame.

## Sources

- [#73](https://github.com/chrischaps/Flint/pull/73) / commit
  [`4877c38`](https://github.com/chrischaps/Flint/commit/4877c384d11d6e406e6c8fba39ad3c18d6e9db27)
  "Engine: material.cast_shadows turns a mesh's shadow off (ADR 0100)"
- Vault: "The Capitol - mist casts no shadow (GH-232, ADR 0100) - 2026-10-20",
  "2026-10-21 Capitol Ranges Move Out With The World"
- Code now: `crates/flint-render/src/scene_renderer/extract.rs`,
  `crates/flint-render/src/scene_renderer/render_passes.rs`,
  `schemas/components/material.toml`, `crates/flint-render/tests/runtime_mesh.rs`
- Book: [Lighting](../book/src/concepts/lighting.md), "Turning casting off"
