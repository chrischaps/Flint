# ADR 0034: World II conducts the path, never the camera

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from Vault note "Star Child World II - The
Undertow - 2026-08-17" and commit `86d21ec`.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

World II, the Undertow, is the most world-controlled chapter: "a world that
acts upon you". The prologue had a constraint that the stick's direction and
the screen's stay in agreement. Only the Vault cites this number.

## Decision

From the Vault note: the scene conducts the *path*, never the camera, so the
prologue's stick-to-screen constraint survives this chapter by construction.
In the same scene, sway is bracing, grip is a calming surge shake, and idle
is "carried by the current", the one place the fiction makes idleness more
correct than in the prologue.

The engine commit lists its items in ADR order, and at this position is
"conducted-frame extensions": `ConductedFrame` gained `sway`, `pressure_l`
and `pressure_r` (zeros under the `prototype` verb map) beside `cues`, and
Rhai gained `conducted_sway()`, `conducted_pressure_l()` and
`conducted_pressure_r()`. That these extensions belong to this record is an
inference from the order; the binding itself lives in the game's scripts.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The rest of the original record did not survive.
- The Vault note numbers the World II plan's records from 0028 to 0034,
  while the engine commit numbers its batch from 0030 to 0037. Numbers 0028
  and 0029 are cited nowhere in Flint, and what they decided is unknown.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Vault: "Star Child World II - The Undertow - 2026-08-17"
- Code now: `crates/flint-music/src/chart_session.rs` (`ConductedFrame`),
  `crates/flint-script/src/api/conducted.rs` (`conducted_sway`,
  `conducted_pressure_l`, `conducted_pressure_r`),
  [Scripting](../book/src/concepts/scripting.md)
