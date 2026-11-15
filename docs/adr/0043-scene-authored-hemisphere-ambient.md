# ADR 0043: A scene can author the hemisphere ambient in `[environment]`

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0` and its diff, Vault note
"Clay as a Material for Feelings - 2026-08-19", and the book.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The shader's hemisphere ambient was hardcoded: a cool sky
`[0.12, 0.13, 0.18]` and a dark ground `[0.06, 0.05, 0.04]`. In the clay-look
spike ([ADR 0042](0042-clay-look-research-spike.md)) that cool blue deadened every shadow side (Vault). Nothing
cites 0043 by number; the commit lists "ambient hemisphere override" first
among its engine levers, and 0044 and 0045 follow it in order, so this
number is inferred.

## Decision

From the commit's diff:

- `[environment]` takes `ambient_sky` and `ambient_ground`, linear RGB.
  Absent keeps the built-in colours, now named
  `LightUniforms::DEFAULT_AMBIENT_SKY` and `DEFAULT_AMBIENT_GROUND`. Giving
  one fills the other from its default.
- `SceneRenderer::set_ambient(sky, ground)` stores the override, so the
  zero-light fallback in `extract_lights_from_world` can't overwrite it.
- `reset_ambient()` clears it (and the diffuse wrap, [ADR 0044](0044-diffuse-wrap.md)). The player
  calls it on every scene transition before applying the new scene's
  values, so one scene's ambient never leaks into the next.
- The player and `flint render` both apply it. A scene that never sets it
  renders as before.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- With diffuse wrap, it is one of the two changes the Vault note says
  mattered most for the clay look.
- Since then: it is one of six `[environment]` levers that share the rule
  "absent or zero is the exact original shading" (with diffuse wrap,
  Oren-Nayar and sheen from [ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md)). The F4 Rendering & Effects menu's
  Lighting section exposes them with a Reset button, and
  `SceneRenderer::lighting_levers()` reads them back. Scripts have no
  binding for them.

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs [0042](0042-clay-look-research-spike.md)-[0047](0047-viewer-dof-follow.md))"
- Vault: "Clay as a Material for Feelings - 2026-08-19"
- Code now: `crates/flint-render/src/pipeline.rs` (`LightUniforms`),
  `crates/flint-render/src/scene_renderer/mod.rs` (`set_ambient`,
  `reset_ambient`, `apply_ambient_override`),
  `crates/flint-scene/src/format.rs` (`EnvironmentDef`),
  `crates/flint-player/src/player_app/scene_entry.rs`,
  [Lighting](../book/src/concepts/lighting.md), "The environment block"
