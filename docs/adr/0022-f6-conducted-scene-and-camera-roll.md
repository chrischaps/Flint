# ADR 0022: The F6 scene is conducted through its environment; scripts can roll the camera

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4`, Vault notes "2026-08-17
Starchild F6 - The World Becomes the Interface" and "Neutral-reset
transitions are not events (2026-08-17)", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Star Child's F6 milestone moved the prototype from a harness shader in a
debug window into a scene. The commit that landed it covered five features
under ADRs 0022-0027; this one is the scene, and the one engine change it
needed.

## Decision

From the commit:

- The prototype scene is conducted by its environment: spline ribbons, a
  generated sky, beacon and presence lights, with the whole conducted
  binding in the game's `scripts/scene_prototype.rhai`.
- The engine gains `set_camera_roll`, and a frame with no roll asked for
  resets the camera to world up (`None` means level).

The Vault note for that day records the split the scene settled on: the
ladder owns desaturation, blur and chromatic aberration; the script owns
exposure, fog, vignette and the camera. It describes this as "what failing
feels like" (engine-tuned, pinned by tests) against "what the world is like"
(authored, hot-reloadable). Whether the original record stated that split
did not survive.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The reset to world up is the point of the `None` case: a roll request
  lasts one frame, so a script that stops asking (a hot reload into a
  compile error) doesn't leave the camera tilted. The Vault lists it with
  other "edges at the boundary of existence" bugs.
- The commit left the look for Chris to review ("[H] look review pending").
- Since then: `set_camera_roll(radians)` is declared in
  `crates/flint-script/src/api/camera.rs` and writes `FrameRequests`
  (`CameraOverrides::roll`). `apply_camera_overrides` in
  `crates/flint-player/src/player_app/script_commands.rs` rebuilds the up
  vector from the view basis each frame a roll is asked for, keeps the old up
  when looking straight up or down, and is shared by the player and
  `flint render --scripts`. The section header `Camera roll override (ADR
  0022)` in `crates/flint-script/src/api/conducted_tests.rs` now has no test
  under it; the round-trip test is `test_camera_roll_override_round_trip` in
  `host_io_tests.rs`.

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "2026-08-17 Starchild F6 - The World Becomes the Interface"
- Vault: "Neutral-reset transitions are not events (2026-08-17)"
- Code now: `crates/flint-script/src/api/camera.rs` (`set_camera_roll`),
  `crates/flint-script/src/context.rs` (`CameraOverrides::roll`),
  `crates/flint-player/src/player_app/script_commands.rs`
  (`apply_camera_overrides`), [Scripting](../book/src/concepts/scripting.md)
