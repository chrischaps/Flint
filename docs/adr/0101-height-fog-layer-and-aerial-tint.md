# ADR 0101: Fog gets a height layer that works, and an aerial tint

Status: accepted, 2026-10-20 ([#74](https://github.com/chrischaps/Flint/pull/74)).
Backfilled 2026-11-12 (#374) from PR #74, whose body holds the ADR text,
commit `6bbb1d8`, and the Vault note "2026-10-20 Capitol Far Hills Turn Blue
In The Air".

## Context

Flint's fog was one exponential in distance, with one colour. Two gaps
showed in The Capitol (TheCapitol#233):

1. Height fog did nothing. It computed `exp(-falloff × h) × dist_fog` and
   returned `max(dist_fog, height_fog)`; the exponential is at most 1, so the
   max always took the distance fog, and `fog_height_enabled` changed nothing
   on screen.
2. The fog had one colour, so far land couldn't turn blue while the fog
   still matched the sky's horizon. The Capitol baked a slate blue into its
   land's vertices by distance from the camera and rebuilt chunks as the
   camera moved. Games that wanted ground mist had to use geometry.

## Decision

- **Height fog is a layer of air.** Its density is `fog_height_density` at
  `fog_height_origin`, falling by e every `1/fog_height_falloff` m above it
  (rising below it). It's integrated in closed form along each view ray,
  `density_at_eye × d × (1 − e^−x)/x` with `x = falloff × Δy`, and added to
  the distance fog's optical depth: `fog = 1 − exp(−(fog_density·max(d −
  fog_start, 0) + height_depth))`. It isn't held off by `fog_start`. One
  `exp` and a divide a pixel; the exponents are clamped.
- **An aerial tint:** `fog_tint_color`, `fog_tint_strength`, `fog_tint_start`
  and `fog_tint_end`. Each drawn pixel is mixed toward the tint by `strength ×
  smoothstep(start, end, d)`, before the fog, so the farthest things still
  run into `fog_color`. It rides `fog_enabled` and leaves the sky alone.
- Both opt-in: `fog_height_density` and `fog_tint_strength` default to 0, so
  a scene that only turned height fog on looks as it did.
- Two 16-byte rows appended to `PostProcessUniforms` (272 to 304 bytes);
  every earlier offset unchanged. Scene fields in `PostProcessDef`, and the
  render panel's Fog section.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Height fog is visible: thick low down, thinning upward, lit like the
  distance fog.
- It's one horizontal slab for the whole scene, so mist that keeps to
  valleys at different heights still needed geometry. The record named fog
  volumes or a layer following a height field as the next step. Since then:
  [ADR 0106](0106-ground-fog-on-own-floors.md) adds a ground fog on floors of
  its own, which grew the uniforms to 368 bytes.
- The tint is light in the air, not a surface colour, so a game keys it with
  the hour as it does `fog_color`. It reaches everything at a distance alike;
  a tint on some things only isn't possible.
- The Capitol's far blue became `fog_tint_*` (strength 0.6 over 120 to
  800 m), dropping the baked haze and its chunk rebuilds.

## Sources

- [#74](https://github.com/chrischaps/Flint/pull/74) / commit
  [`6bbb1d8`](https://github.com/chrischaps/Flint/commit/6bbb1d81634eacf8b08f02606cc8625bf9eb44fb)
  "Fog: a height layer that works, and an aerial tint (ADR 0101)"
- Vault: "2026-10-20 Capitol Far Hills Turn Blue In The Air"
- Code now: `crates/flint-render/src/composite_shader.wgsl`
  (`height_fog_depth`), `crates/flint-render/src/postprocess.rs`
  (`PostProcessConfig`), `crates/flint-scene/src/format.rs`
- Book: [Post-processing](../book/src/concepts/post-processing.md), fog;
  [Formats](../book/src/formats/overview.md)
