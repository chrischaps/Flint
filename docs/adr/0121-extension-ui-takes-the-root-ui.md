# ADR 0121: `PlayerExtension::ui` takes the frame's root `Ui`

Status: accepted, 2026-11-14 (#959). Amends
[ADR 0071](0071-player-extensions-in-a-game-binary.md) and
[ADR 0073](0073-game-egui-layer-and-input-routing.md).

## Context

`PlayerExtension::ui(&egui::Context)` ([ADR 0071](0071-player-extensions-in-a-game-binary.md),
called from [ADR 0073](0073-game-egui-layer-and-input-routing.md) on) was where a game drew its
own egui. egui 0.36 shows its one `Panel` type only on a `Ui`, so after the
wgpu 30 upgrade (#950) a game's side or top panel had no port. #946 added a
second hook, `root_ui(&mut egui::Ui)`, which the player called with the
frame's root `Ui` and whose default called `ui(ui.ctx())`. Nothing broke, but
it left two overlapping hooks under names egui doesn't use. #959 asked for
the shape egui 0.36 itself means a host to hand its UI over in.

What egui and eframe say, from their source in the cargo registry
(`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`) and docs.rs:

- **egui hands out a `Ui`, not a `Context`.** `Context::run_ui(input,
  impl FnMut(&mut Ui))` is the one way to run a frame
  (`egui-0.36.2/src/context.rs:794`). It builds the root `Ui` on the
  background layer over the viewport, with no margin or background. egui
  0.34 deprecated `Context::run(.., impl FnMut(&Context))` with "Call run_ui
  instead" (`egui-0.34.3/src/context.rs:844`); 0.36 has no `run`.
- **Panels are shown on a `Ui`.** `Panel::show(self, ui: &mut Ui, ..)`
  (`egui-0.36.2/src/containers/panel.rs:422`), and the module doc says a
  panel is either a child of a `Ui` or top-level, taking part of the whole
  screen. 0.34 deprecated the `Context` form ("Use show_inside() instead",
  `egui-0.34.3/src/containers/panel.rs:519`); 0.36 removed it and renamed
  `show_inside` to `show`. `SidePanel` and `TopBottomPanel` were type
  aliases of `Panel` in 0.34 (`panel.rs:1101`, `:1104`) and are gone.
- **Windows and areas still take a `&Context`, and a `Ui` is one.**
  `Window::show(self, ctx: &Context, ..)` (`containers/window.rs:543`),
  `Area::show(self, ctx: &Context, ..)` (`containers/area.rs:406`). `impl
  Deref for Ui { type Target = Context; }`, documented "Allow using `Ui`
  like a `Context`" (`egui-0.36.2/src/ui.rs:92`). So code given the root
  `Ui` shows a window with `Window::new(..).show(ui, ..)` unchanged. egui's
  own panel code calls `Context` functions with a `Ui` the same way
  (`animate_expansion(ui, ..)`, `panel.rs:457`).
- **eframe's per-frame method takes the `Ui`.** eframe 0.34 made
  `App::ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)` the required
  method and deprecated `App::update(&mut self, ctx: &egui::Context, ..)`
  with "Use Self::ui instead" (`eframe-0.34.3/src/epi.rs:176`, `:189`),
  calling both for one release (`native/epi_integration.rs:287-299`).
  eframe 0.35 and 0.36 have no `update`: `ui` is the one required method
  (docs.rs, `eframe/0.35.0` and `eframe/0.36.2`, `trait.App.html`; eframe
  0.36 isn't in the registry here). 0.37 is the same
  (`eframe-0.37.0/src/epi.rs:152`). Work that shows no UI goes in
  `App::logic(&mut self, ctx: &egui::Context, ..)`, which takes a `Context`.
- **egui's own plugin hook made the same move.** `Plugin::on_begin_pass`
  and `on_end_pass` took `&Context` in 0.33 (`egui-0.33.3/src/plugin.rs:27`)
  and take `&mut Ui` from 0.34 (`egui-0.34.3/src/plugin.rs:27`,
  `egui-0.36.2/src/plugin.rs:27`), the root `Ui` `run_ui` builds. This is
  egui's own extension point for a component drawn into someone else's
  frame, and it changed signature in place, with no deprecation.
- No egui demo crate, bevy_egui or egui example is in the registry here.
  egui-winit and egui-wgpu 0.36.2 never call `run_ui`: they leave the frame
  to the host.

## Decision

1. **`PlayerExtension::ui(&mut self, ui: &mut egui::Ui)`.** The player
   calls it with the root `Ui` from `run_ui`, where it called the old `ui`:
   after script UI, before the debug panels. A game shows panels on it and
   windows and areas from it, and `ui.ctx()` is the `Context` to clone.
   This is eframe's `App::ui` without the `Frame`, and egui's
   `Plugin::on_begin_pass`.
2. **`root_ui` is deprecated and still called.** The frame calls
   `extension::draw_ui`, which calls each extension's `root_ui`, whose
   default calls `ui`. A game that overrode `root_ui` after #946 keeps
   drawing. The note says what to write: rename `fn root_ui` to `fn ui`. It
   goes at the next engine change that breaks games anyway.
3. **No separate logic hook.** eframe's `App::logic(&Context)` exists to run
   while the window is hidden; the player already has `pre_update` and
   `update(Stage, ..)` for work that draws nothing.

## Alternatives considered

- **Keep `root_ui` beside a deprecated `ui(&Context)`.** Nothing breaks, but
  the hook eframe calls `ui` stays named `root_ui` for good, or takes a
  second break when it's renamed. Two hooks also make every reader ask which
  to override.
- **A new name (`egui_ui`, `draw_ui`) taking `&mut Ui`, with `ui(&Context)`
  deprecated.** It gives a deprecation path for `ui`, but the name matches
  neither eframe nor egui, and it is still the second hook #959 set out to
  remove.
- **`ui(&mut self, ui: &mut Ui)` with `root_ui` deleted.** `root_ui` has
  been on `main` since the same day, and no game on this machine or in
  chrischaps' repositories implements it (`gh search code`), but a game
  that pulled in between would fail to compile. Deprecating costs one
  default method.
- **Keep `&Context` and hand panels a `Ui` some other way** (a
  `Context`-to-root-`Ui` lookup). egui has none: the root `Ui` exists only
  inside `run_ui`'s closure.

## Consequences

- **A game's old `ui` no longer compiles,** and it can't be deprecated: a
  trait method has one signature, and changing it is error E0053
  (``method `ui` has an incompatible type for trait``). The port is the
  parameter's type. With `fn ui(&mut self, ctx: &mut egui::Ui)` the body
  compiles as it was: `Window::new(..).show(ctx, ..)`, `ctx.input(..)`,
  `ctx.clone()` (a `Context`: `Ui` isn't `Clone`) and passing `ctx` to a
  `fn(&egui::Context)` all go through the `Deref`. The test
  `a_games_floating_window_is_shown_through_ui` holds a body written that
  way. The one game known to implement `ui`, The Capitol, takes this with
  the egui 0.36 port it needs anyway.
- **rustc doesn't warn on implementing a deprecated trait method,** only on
  calling one (checked on rustc 1.99.1). A game still overriding `root_ui`
  hears about it from the changelog and the book, not the compiler.
- A game's panels sit on the background layer with the script UI, after it,
  so in front; its windows float on Middle; the debug panels stay on
  Foreground over both.
- `crates/flint-player/examples/cave_game` draws a side panel and a window through the hook.
- The player's internal `Subsystem::ui(&egui::Context)`
  ([ADR 0117](0117-player-subsystems-and-one-game-step.md)) keeps its `Context`: it isn't public,
  and its one client draws windows.

## Sources

- #959, #946 (PR #956's body explains `root_ui`), #945
- Code: `crates/flint-player/src/extension.rs` (`PlayerExtension::ui`,
  `root_ui`, `draw_ui`), `crates/flint-player/src/player_app/frame.rs`,
  tests in `crates/flint-player/src/player_app/extension_tests.rs`; book:
  [Player Extensions](../book/src/guides/player-extensions.md), "Game UI with egui", and
  [Upgrading the Engine](../book/src/guides/upgrading-the-engine.md), "The game UI hook"
