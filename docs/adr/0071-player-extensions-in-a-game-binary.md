# ADR 0071: A game binary installs player extensions into `PlayerApp`

Status: accepted, 2026-10-11 ([#41](https://github.com/chrischaps/Flint/pull/41)).
Backfilled 2026-11-12 (#374) from commit `4a0adfc`, PR #41's description and
the Vault notes of The Capitol's P1 hooks (H1-H5).
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The Capitol's 3D client was planned as a game crate that wraps `PlayerApp`,
as `flint-android` already did. The plan found that Flint had no way for a
game to register native Rhai functions, `GameEvent::Custom` never reached
scripts, and egui got input only while a debug panel was open. It listed
five general hooks to go upstream, H1-H5, which became ADRs 0071-0075. The
P0.4 spike wrapped `PlayerApp` in its own `ApplicationHandler` and forwarded
events to it.

## Decision

- `PlayerExtension`, a trait in `flint-player/src/extension.rs`, installed
  with `PlayerApp::add_extension`. Its four methods default to no-ops:
  - `register_script_api(&mut rhai::Engine)`, called at `add_extension`.
    The path is `ScriptSystem::rhai_engine_mut()`. A registration with the
    same signature replaces Flint's own function, and registrations survive
    `clear()`, so they last across scene transitions.
  - `pre_update(&mut ExtensionCtx)`, in add order, after the clock tick,
    the input script and the transition, and before game events and
    `on_update`. World writes are visible to scripts in the same frame.
  - `on_window_event(&WindowEvent) -> bool`, after open debug panels and
    before the player. Only input events can be consumed; lifecycle events
    always reach the player.
  - `ui(&egui::Context)`, declared here and called from [ADR 0073](0073-game-egui-layer-and-input-routing.md) on.
- `ExtensionCtx` holds `world`, `clock`, `input`, `camera` and
  `gpu: Option<ExtensionGpu { renderer, device, queue }>`, there once the
  renderer exists.
- A player with no extensions behaves exactly as before.

## Alternatives considered

- A wrapper `ApplicationHandler` in the game binary, as the spike had. The
  extension covers what it was for (the network drain and UI routing), so
  the client runs `PlayerApp` directly.
- Three separate options for renderer, device and queue, instead of one
  `gpu` option grouping them.

The fuller reasoning didn't survive.

## Consequences

- Game Rust and Flint scripts meet in one frame order, testable headless:
  `PlayerApp::new(FlintWorld::new(), ...)` plus `app.tick()` runs with no
  window (`player_app/extension_tests.rs`).
- The extension is the place later hooks hang from. Since then:
  `ExtensionCtx` gained `persist`
  ([ADR 0080](0080-extensions-reach-the-persistent-store.md)), `audio` ([ADR 0084](0084-extensions-reach-the-audio-engine.md)), `rays`
  ([ADR 0105](0105-rays-against-drawn-meshes.md)) and `scene_tables` (#686); the trait gained `fonts`
  ([ADR 0081](0081-project-fonts-name-egui-defaults.md)), `on_key`
  ([ADR 0103](0103-extensions-see-scripted-keys.md)), `debug_panels` / `debug_panel_ui` ([ADR 0107](0107-debug-panels-from-the-game.md)),
  `declare_script_api` ([ADR 0116](0116-declared-script-api.md)) and
  `register_generators`
  ([ADR 0119](0119-game-generators-through-player-extensions.md)).

## Sources

- [#41](https://github.com/chrischaps/Flint/pull/41) / commit
  [`4a0adfc`](https://github.com/chrischaps/Flint/commit/4a0adfc2c417985c67f6c0a0f2066bb259f39ec0)
  "[P1.1] H1: PlayerExtension trait and ExtensionCtx (Flint)"
- Commit
  [`2e73d04`](https://github.com/chrischaps/Flint/commit/2e73d040a970e4428d09890cba5a998eb30eaeb7)
  "[P1.6] Player extensions: ADR citations and a book guide (Flint)"
- Vault: "2026-10-11 Capitol Flint Client PlayerExtension Hook",
  "2026-10-11 Capitol Flint Client Planned", "2026-10-12 Capitol Flint
  Hooks Go Upstream As PR 41"
- Code now: `crates/flint-player/src/extension.rs` (`PlayerExtension`,
  `ExtensionCtx`); book:
  [Player Extensions](../book/src/guides/player-extensions.md)
