# ADR 0051: The scene viewer opens on the scene's authored `[camera]`

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from the Vault note "2026-08-19 Clay Look Orbit
Variant", the flint-cli CLAUDE.md line that cites it and the code commit
`aac2d27` added. No commit message names ADR 0051.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The scene viewer applied a scene's authored `[post_process]` on load (since
[ADR 0046](0046-viewer-applies-scene-post-process.md)) but ignored its `[camera]`. A scene with strong depth of field
opened as a blur at the viewer's default distance, and `flint edit` never
showed the framing the evidence PNGs were shot from. The clay spike also
needed to be judged in the round, as a claymation maquette is.

## Decision

- The viewer seeds its orbit camera from the scene's `[camera]`:
  `target`, `fov`, `near`, `far`, and distance, yaw and pitch derived from
  `position` and `target` the way the player does
  (`seed_camera_from_scene`). Space returns to that framing.
- `flint edit --auto-orbit` now covers scenes, not only models and procgen
  specs: a turntable with O to toggle and `[` / `]` for speed, like the model
  previewer. Any manual orbit cancels it.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The Vault note puts it as "post and camera are one composition".
- The flint-cli CLAUDE.md line calls it "ADR 0051 in game repos using this
  pattern", the only place in this repo that names it.
- Since then: auto-orbit centres on the selected entity (particles work,
  [ADR 0068](0068-particle-effect-assets-and-deterministic-sim.md)).

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))", which carries the code
- Vault: "2026-08-19 Clay Look Orbit Variant"
- Code now: `crates/flint-viewer/src/app.rs` (`seed_camera_from_scene`,
  `auto_orbit`), `crates/flint-cli/src/commands/edit_router.rs`
  (`EditArgs::auto_orbit`)
- Book: [Viewing](../book/src/getting-started/viewing.md)
