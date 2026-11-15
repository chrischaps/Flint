# ADR 0059: The clay prologue uses one neutral clay set, tinted in the scene

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27` and the Vault note
"2026-08-20 Clay tint gotcha - model entities read base_color scalars".
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A Starchild game decision. Commit `aac2d27` lists ADRs 0059-0061 as
"prologue variants — clay neutral-tint set, skinned stick figures, flight
trails". The Vault note of that day built the clay-look variant of the
Star Child prologue, reusing the clay spike's assets.

## Decision

From the Vault note: the clay prop generator takes a `--tub HEX` flag, so
one neutral clay set is generated and each prop's hue is decided in the
scene. What the material is (pocks, thumb dents, wobble) is kept apart from
its colour. The full record's reasoning did not survive.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The note reports eighteen props became clay in an afternoon without
  touching the choreography script.
- A model entity is tinted through the scalar `material.base_color_r/g/b`
  fields. The array form `material.color = [r, g, b, a]` that bounds-box
  entities use did nothing on a GLB entity, which is why the first clay
  render came back grey. Scripts tint models through the same scalars.
- The generator and the scenes are in the Starchild repo; nothing in Flint
  cites ADR 0059.
- Since then: the scalars are read through the typed view, so an integer
  channel counts (#685), and a model can also be tinted per glTF material
  through `material_overrides`.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Vault: "2026-08-20 Clay tint gotcha - model entities read base_color
  scalars"
- Code now: `crates/flint-render/src/scene_renderer/extract.rs`
  (`ecs_base_color_override`)
