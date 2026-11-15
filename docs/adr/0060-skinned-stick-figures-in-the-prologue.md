# ADR 0060: Skinned stick figures in the prologue (decision not recovered)

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27`, the only source that
names it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A Starchild game decision. Commit `aac2d27` lists ADRs [0059](0059-clay-prologue-neutral-tint-set.md)-[0061](0061-flight-trails-from-script-driven-emitters.md) as
"prologue variants — clay neutral-tint set, skinned stick figures, flight
trails". Taken in order, ADR 0060 is the skinned stick figures.

## Decision

The record's text did not survive. Nothing in Flint's code, book or Vault
cites ADR 0060.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- In the same commit, skinned draws got the static model path's tint:
  `material.base_color_r/g/b` on the entity or its parent overrides the
  glTF colour (`ecs_base_color_override`), "so a scene can tint a skinned
  figure". Whether that change belongs to this decision isn't recorded.
- The Vault has ModelGen notes from the same day on box-modelling a stick
  figure in Blender and keying its walk, idle and flying clips. They don't
  name the prologue or ADR 0060.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Vault: "2026-08-20 ModelGen - Box Modelling a Stick Figure"
- Code now: `crates/flint-render/src/scene_renderer/extract.rs`
  (`ecs_base_color_override`)
