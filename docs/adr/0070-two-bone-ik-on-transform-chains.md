# ADR 0070: Two-bone IK on transform chains, solved by the engine each frame

Status: accepted, 2026-09-05 (commit
[`f3fc3a5`](https://github.com/chrischaps/Flint/commit/f3fc3a5008aa8627fa9a585b6d9afdc2910fabb3)).
Backfilled 2026-11-12 (#374) from commit `f3fc3a5`, the Vault note "Trike
Pilot Steers And Leans With Engine IK" (2026-09-05), and the code and book
lines that cite it. This record was never a file; it lived in the commit
message and code comments.

## Context

The Trike game's pilot became sixteen glTF nodes that turn the yoke with
both arms and lean with the steering and the `pitch` input. The arms had to
follow the yoke. The model is a node hierarchy expanded by an `animator`,
not a skinned skeleton.

## Decision

- An `ik_two_bone` component on the *second* bone of a transform chain
  (forearm, shin). Its transform parent is the first bone, and it names a
  `target`, a `pole` and a `tip` entity, with `weight` and `enabled`.
- `IkPass` in `flint-animation` solves every armed chain each frame after
  scripts, physics and animation have posed the world, and before the
  renderer reads world matrices. `flint render` runs it once.
- The solve writes only the two bones' `rotation_quat`, restarting from
  each bone's captured rest rotation every frame, so twist cannot drift.
  Segment lengths come from the model's rest offsets, so a glTF that
  carries the IK empties needs no numbers in the scene.
- A target past reach straightens the chain toward it. Bones on the chain
  must have unit scale.
- The math is in `flint-core`: `solve_two_bone` in `ik.rs`, and
  `quat_from_two_vectors` and `quat_nlerp` beside the other quaternion
  helpers.

## Alternatives considered

From the Vault note, Chris's call: add IK to Flint rather than solve the
arms in Rhai. It can be reused for rigged characters later, it keeps the
script as simple as a `set_rotation`, and the model carries the targets.

## Consequences

- A script arms IK with `set_field(id, "ik_two_bone", "target", name)` and
  writes only the yoke and the torso.
- World matrices are computed on demand and never cached, so any write
  before the renderer syncs is seen.
- The same commit carried unrelated work from a parallel session: GPU
  adapter selection (`gpu_select.rs`, `--gpu`) and a docs correction on
  scripts running before physics.
- Since then: the player runs the pass from `GameCore::after_animation`
  ([ADR 0117](0117-player-subsystems-and-one-game-step.md)), and
  `flint render --anim-time` through `HeadlessAnimation` (#307); both call
  `AnimationSystem::run_ik`.

## Sources

- Commit
  [`f3fc3a5`](https://github.com/chrischaps/Flint/commit/f3fc3a5008aa8627fa9a585b6d9afdc2910fabb3)
  "Pilot steers and leans; engine two-bone IK (ADR 0070)"
- Vault: "2026-09-05 Trike Pilot Steers And Leans With Engine IK"
- Code now: `crates/flint-core/src/ik.rs`,
  `crates/flint-animation/src/ik_pass.rs` (`IkPass`),
  `crates/flint-player/src/player_app/game_core.rs`,
  `crates/flint-player/src/player_app/headless_animation.rs`,
  `schemas/components/ik_two_bone.toml`
- Book: [Animation](../book/src/concepts/animation.md), "Two-Bone IK"
