# ADR 0061: Flight trails from script-driven particle emitters

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27` (message and diff),
commit `98886da` and the code and book lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A Starchild prologue variant: commit `aac2d27` lists ADRs 0059-0061 as
"clay neutral-tint set, skinned stick figures, flight trails (+
gen_trail_sprites tool ...)". The tool and the scenes are the game's. The
engine half is in `flint-particles`, and the code comments that cite ADR
0061 are all on it.

## Decision

What the commit's diff gave the inline `particle_emitter`. Code comments
cite ADR 0061 on the first three and the last; the local-space change is in
the same diff without a citation.

- `shape_offset`: the spawn region's translation from the emitter, so a
  script can park it on one side of a body. Default zero.
- `shape_axis_u` / `shape_axis_v`: an oriented box. `extents.x` runs along
  `u`, `extents.y` along `v`, `extents.z` along their cross product. Zero
  or parallel axes keep the axis-aligned box verbatim.
- The emission-shape fields a script drives per frame (direction, spread,
  speeds, lifetimes, stretch, shape, offset and axes) are copied into the
  running emitter each sync, "e.g. a trail that sheds in the direction of
  lateral motion".
- Local-space particles are resolved to world space at pack time, since the
  render path has no per-emitter transform.
- All of it in world-space axes: the offset and box axes don't turn with the
  entity.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Since then: the particle rebuild ([ADR 0068](0068-particle-effect-assets-and-deterministic-sim.md))
  added `local_axes`, which rotates direction, offset and shape axes with
  the entity and scales distances by its scale. Effect assets default it to
  `true`; inline components default to `false` to keep this behaviour,
  which the Trike game's inline trails rely on. The Vault notes those
  trails were not smoke-tested after the rebuild.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs 0053-0061)"
- Commit
  [`98886da`](https://github.com/chrischaps/Flint/commit/98886daa5ab190590c498193c6178f1865beb08e)
  "particles: effect assets, deterministic sim, editor, viewer/player
  integration ([ADR 0068](0068-particle-effect-assets-and-deterministic-sim.md))"
- Vault: "2026-09-04 Flint Particles V2 Effect Assets And Editor"
- Code now: `crates/flint-particles/src/emitter.rs` (`shape_offset`,
  `shape_axis_u`, `local_axes`), `schemas/components/particle_emitter.toml`
- Book: [Particles](../book/src/concepts/particles.md)
