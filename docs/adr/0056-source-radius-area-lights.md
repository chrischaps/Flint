# ADR 0056: Lights get a source size: `source_radius` and `angular_size`

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27`, commit `647cc41` and
the code and book lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Part of the Tier 2 renderer pass ([ADR 0054](0054-tier-2-renderer-pass-scope.md)),
listed as "source-radius area lights". Every light was a point, so the
highlight of a point or spot light on a glossy surface was a pinprick.

## Decision

- Point and spot lights take `source_radius` (metres); directional lights
  take `angular_size` (degrees, stored in radians: the sun is about 0.5, a
  softbox 2-5).
- Specular uses the representative-point approximation: a source of radius
  `r` seen from distance `d` widens roughness by about `r / (2d)`, clamped
  to 1, and the shading distance never falls inside the source. Diffuse is
  unchanged.
- 0 is a point and takes the earlier code path verbatim.
- `PointLight` and `SpotLight` gain a `source_radius` field, mirrored in
  the shader, skinned, terrain, grass, volumetric and ocean WGSL structs.
- A directional light's `angular_size` takes its former `_pad1` slot, so
  that layout is unchanged; no shader reads it directly. It reaches the
  shadow code as `tan(angular_size)` and drives the PCSS penumbra
  ([ADR 0057](0057-pcss-contact-hardening-shadows.md)).

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Lights that don't set the fields render as before.
- `PointLight` grew from 32 to 48 bytes, and `LightUniforms` (16 point
  lights) from 1280 to 1536.
- Since then: both fields are in `schemas/components/light.toml`, which a
  test holds to what `extract_lights_from_world` reads
  ([ADR 0115](0115-scene-diagnostics-and-complete-schemas.md)).

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Commit
  [`647cc41`](https://github.com/chrischaps/Flint/commit/647cc416c22c96fb667c9b4bb60d9c884d155037)
  "docs: new pages for lighting, music sessions, procgen, debug panels,
  music CLI"
- Code now: `crates/flint-render/src/shader.wgsl` (point and spot loops),
  `crates/flint-render/src/pipeline.rs` (`PointLight`, `SpotLight`),
  `crates/flint-render/src/scene_renderer/mod.rs`,
  `schemas/components/light.toml`
- Book: [Lighting](../book/src/concepts/lighting.md), "Area Lights"
