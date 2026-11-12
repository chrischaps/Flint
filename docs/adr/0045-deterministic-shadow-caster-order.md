# ADR 0045: The strongest directional light casts the shadows, with name order breaking ties

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0` and its diff, the book's
lighting chapter, and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Only directional slot 0 gets cascaded shadows. `extract_lights_from_world`
iterated the light component index, a `HashSet` whose order changes per
process, so which light took slot 0, and so cast the shadows, could change
from one run to the next. The commit's comment says this "silently
reassigned" the shadow caster between runs.

## Decision

From the commit's diff:

- Light entities are sorted by entity name, then id, before the uniforms
  are filled, so light order is stable and follows the authored names.
- Then the directional lights are stable-sorted by `intensity`, highest
  first. The strongest directional casts the shadows (sun over fill),
  whatever the names, and name order breaks ties.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A fill light can't take the sun's shadows because of how it was named
  (book).
- The order is the same every run, which fits Flint's other rule that
  anything reproducible derives from entity names, never ids.
- Since then: a hidden entity's light is dropped before the sort (#628).
  The two sorts are unchanged.

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs [0042](0042-clay-look-research-spike.md)-[0047](0047-viewer-dof-follow.md))"
- Code now: `crates/flint-render/src/scene_renderer/mod.rs`
  (`extract_lights_from_world`),
  [Lighting](../book/src/concepts/lighting.md)
