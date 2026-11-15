# ADR 0048: Oren-Nayar diffuse and a tinted sheen as `[environment]` levers

Status: accepted, 2026-08-19 (commit
[`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)).
Backfilled 2026-11-12 (#374) from commit `95a260a`, commit `647cc41`, the
code comments that cite it and the Vault notes on the clay-look spike.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Starchild was trying a clay look for its prologue (`spikes/clay_look/`, in
the game repo). Tier 0 of that work changed scene content only; Tier 1 was
six engine levers, each gated so that it changes nothing until a scene asks
for it. Commit `95a260a` landed all six and cites ADRs 0048-0050 as their
records. This one covers the two shading levers. The Vault note from that day
says what made clay read as clay was "warmth at the edge of light": the
hemisphere ambient and the diffuse wrap ([ADR 0044](0044-diffuse-wrap.md)) had come first.

## Decision

- Three keys in `[environment]`: `oren_nayar` (0-1), `sheen_color` and
  `sheen_strength`.
- `oren_nayar` blends Lambert diffuse toward the Fujii approximation of
  Oren-Nayar. Sigma comes from the material's roughness; the key only blends.
  The Fujii `s/t` term is clamped, because normal maps at grazing angles
  made fireflies.
- The blend rides `ambient_ground.w` as `1 + oren`, the encoding
  `diffuse_wrap` already used on `ambient_sky.w`, so a legacy write of `1.0`
  decodes to off.
- The sheen is a Charlie sheen rim, tinted. It is a new `vec4` on
  `LightUniforms` (`sheen_color_strength`), not an encoding trick; all zero
  is off. It has no energy compensation.
- `LightUniforms` is mirrored in six WGSL structs (shader, skinned, terrain,
  grass, volumetric, ocean). The commit added the first layout assert,
  `light_uniforms_layout`, which holds them to the Rust struct.
- Absent or zero is the exact earlier shading. No Rhai setters
  ([ADR 0050](0050-color-grade-film-grain-and-fxaa.md)).

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Scenes that never set the keys render as before. The commit's gates
  reported same-binary renders identical with the levers off.
- High `sheen_strength` over-brightens; the book says to keep it at or
  below about 0.3.
- `flint render` takes `--oren-nayar`, `--sheen-color` and
  `--sheen-strength`.
- Since then: the sheen vec4 grew `LightUniforms` from 1264 to 1280 bytes,
  and later levers kept the same rule
  ([ADR 0056](0056-source-radius-area-lights.md),
  [ADR 0057](0057-pcss-contact-hardening-shadows.md)).

## Sources

- Commit
  [`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)
  "Clay-look Tier 1: all six gated levers (ADRs 0048-0050)"
- Commit
  [`647cc41`](https://github.com/chrischaps/Flint/commit/647cc416c22c96fb667c9b4bb60d9c884d155037)
  "docs: new pages for lighting, music sessions, procgen, debug panels,
  music CLI"
- Vault: "Clay as a Material for Feelings - 2026-08-19", "2026-08-19 Clay
  Look Tier 0 Photography Pass"
- Code now: `crates/flint-scene/src/format.rs` (`oren_nayar`,
  `sheen_color`, `sheen_strength`), `crates/flint-render/src/pipeline.rs`
  (`LightUniforms`), `crates/flint-render/src/shader.wgsl`
- Book: [Lighting](../book/src/concepts/lighting.md), "The `[environment]`
  Block" and "The clay look"
