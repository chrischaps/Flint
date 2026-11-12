# ADR 0075: `screen_to_ground` picks the ground under a screen point

Status: accepted, 2026-10-12 ([#41](https://github.com/chrischaps/Flint/pull/41)).
Backfilled 2026-11-12 (#374) from commit `94ae13f`, PR #41's description and
the Vault note "2026-10-12 Capitol Flint Cursor Finds The Ground".
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A game needed to ask what ground lies under the cursor, for click-to-move
and a reticle under the pointer. `flint-script` can't depend on
`flint-render`, so the script side has only the view-projection matrix it
already gets.

## Decision

- `flint_core::projection`: `screen_to_ndc`, `unproject_ray`,
  `ray_hit_plane_y` and `screen_to_ground` on a view-projection matrix, in
  f64 so distant hits stay on the cursor with a far/near ratio of 20,000.
- `Camera::screen_to_ground(x, y, width, height, plane_y)`.
  `Camera::unproject_ray` delegates to the core module.
- Rhai `screen_to_ground(x, y, plane_y)` returns `[x, z]` or `()`, in
  logical points, the exact inverse of `world_to_screen`.
- The camera's view basis falls back to north (-Z) at the top of the screen
  and east (+X) on the right when looking straight down. The view matrix
  used to collapse there and render nothing.
- `flint-player` refreshes the scripts' camera after the script camera
  overrides, so `on_draw_ui` projects with the camera the frame renders
  with. `on_update` still sees last frame's camera.

## Alternatives considered

- A map return in Rhai. It returns `[x, z]`, as the issue specified.
- Working around the straight-down basis. It was fixed, because the tests
  require that pitch and a top-down map view is plausible.

## Consequences

- Behaviour changes for every game: top-down cameras render, and
  `world_to_screen` in `on_draw_ui` matches the rendered frame instead of
  trailing it by one frame. Without the refresh, a following camera made
  HUD reticles swim by a frame.
- Since then:
  [ADR 0087](0087-native-code-projects-with-frame-camera.md) gave native
  functions the same camera through `Projector`.

## Measured

Camera round trips within 2 mm across perspective and ortho, pitches
15-90°, four yaws and far/near = 20,000. In `screen_to_ground.rs`,
`world_to_screen` of a scripted hit lands within 0.01 pt of the cursor.

## Sources

- [#41](https://github.com/chrischaps/Flint/pull/41) / commit
  [`94ae13f`](https://github.com/chrischaps/Flint/commit/94ae13fc9692b07aa3ff64c307c9f4e7b7dac769)
  "[P1.5] H5: screen_to_ground for scripts and Rust (Flint)"
- Vault: "2026-10-12 Capitol Flint Cursor Finds The Ground"
- Code now: `crates/flint-core/src/projection.rs`,
  `crates/flint-render/src/camera.rs` (`screen_to_ground`, the view basis),
  `crates/flint-script/src/projector.rs`; book:
  [Player Extensions](../book/src/guides/player-extensions.md), "Picking
  the Ground"
