# ADR 0087: Native script functions project with the frame's camera: `Projector`

Status: accepted, 2026-10-17 ([#55](https://github.com/chrischaps/Flint/pull/55)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #55's description and
commit `89a10ba`, which called it [ADR 0086](0086-placed-sound-falloff-curve.md). #54 merged first as 0086, and
PR #56 renamed the references to 0087.

## Context

`world_to_screen` and `screen_to_ground` read
`ScriptCallContext::camera_view_proj`, which the host refreshes to the
camera the frame renders with before `on_draw_ui`
([ADR 0075](0075-screen-to-ground-picking.md)). That context is private to
`flint-script`, so a native function an extension registers
(`register_script_api`) couldn't project with it. `ExtensionCtx::camera` in
`pre_update` is the previous frame's camera, so overlays projected there
trail the world by a frame. The Capitol worked round it: `hud.rhai` called
`world_to_screen` once per player and passed an array of maps to a native
function, which read them back.

## Decision

`flint_script::Projector` (re-exported from `flint-player`) is a cloneable
`Send + Sync` handle on the script context. You get one from
`ScriptSystem::projector()` or `PlayerApp::projector()`, and pass it to an
extension before `add_extension`.

- `Projector::view()` takes the lock once and returns
  `View { view_proj, width, height }`, which is `Copy`.
- `View::world_to_screen(p) -> Option<ScreenPoint { x, y, depth }>` is
  `None` exactly where Rhai's says `visible: false`.
  `View::screen_to_ground(x, y, plane_y)` and `View::screen_size()` are
  also provided.
- Rhai's `world_to_screen` goes through `View`, so native code and scripts
  agree to the bit.
- Called from a script, the view is the frame's camera. Called from outside
  a script call (`pre_update`), it's last frame's, like
  `ExtensionCtx::camera`.

## Alternatives considered

- Passing the view-projection to an `on_draw_ui`-time extension hook: more
  API, and functions called from scripts still wouldn't see the camera.
- Making `ExtensionCtx::camera` current: it can't be, because script camera
  overrides apply after `pre_update`.

## Consequences

- No trait change, so existing extensions are unaffected.
- `view()` takes the script context's mutex. Calling it from a callback the
  script system runs while holding that lock (the text measurer, the
  terrain height function) would deadlock, and the docs say so.

## Measured

The workaround cost a native call and a 4-key map per player in each
direction, every frame: about 0.2 ms at 46 players.

## Sources

- [#55](https://github.com/chrischaps/Flint/pull/55) / commit
  [`89a10ba`](https://github.com/chrischaps/Flint/commit/89a10ba44aa5f42ef3d94cd372eb3ec238c7a3e8)
  "Native script functions project with the frame's camera: Projector (ADR
  0086)"
- [#56](https://github.com/chrischaps/Flint/pull/56) / commit
  [`da09c65`](https://github.com/chrischaps/Flint/commit/da09c65a48e4a207b27fd55e6e203711642b20c5)
  "Projector is ADR 0087: #54 (sound falloff) took 0086 first"
- Vault: "The Capitol - native projector (H14, GH-97) - 2026-10-17"
- Code now: `crates/flint-script/src/projector.rs`,
  `crates/flint-script/src/api/view.rs`,
  `crates/flint-player/src/player_app/init.rs` (`PlayerApp::projector`);
  book: [Player Extensions](../book/src/guides/player-extensions.md),
  "Projecting from Native Functions"
