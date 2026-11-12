# ADR 0073: A game egui layer, with egui seeing input first

Status: accepted, 2026-10-12 ([#41](https://github.com/chrischaps/Flint/pull/41)).
Backfilled 2026-11-12 (#374) from commit `e8e355f`, PR #41's description and
the Vault note "2026-10-12 Capitol Flint Game UI Takes The Keyboard".
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.
Amended by [ADR 0097](0097-input-script-keys-reach-egui.md) (input-script keys take the same gate) and [ADR 0110](0110-capture-and-focus-gates.md)
(capture and focus gates), and by commit `ac25c88` (only a text field
counts as typing).

## Context

egui got input only while a debug panel was open, and
`PlayerExtension::ui` ([ADR 0071](0071-player-extensions-in-a-game-binary.md))
was declared but not called. The debug panels were `SidePanel`s, which
always paint on the background layer, under any game `Window`. The player
quit on Escape when the cursor wasn't captured, so a game with a text box
needed Escape kept from it while typing.

## Decision

- `PlayerExtension::ui()` runs every frame inside the player's egui pass.
  Back to front: script UI, game egui, debug panels.
- The debug panels (F3/F4) become fixed Foreground `Area`s, with the same
  layout and a grip on a column's left edge to resize it. The F2 stats
  overlay sits beside the columns and no longer takes clicks.
- Input routing, in `player_app/egui_input.rs` (`EguiInputGate`): with any
  extension installed or any debug panel open, egui sees window events
  first. Keys and IME are consumed while a widget has keyboard focus,
  pointer events by egui-winit's verdict, and Tab only when focused.
- A release follows its press: whichever side got the press gets the
  release and the repeats, so nothing sticks when focus moves mid-press.
- Escape in a focused text field unfocuses it instead of quitting.

## Alternatives considered

Not recorded in the surviving text. The note records two choices: the
panels moved to Foreground so they are on top, not only on top within the
background layer; and routing is opt-in, so plain Flint games behave as
before.

## Consequences

- A game's egui windows can't cover the debug panels.
- egui-winit reports Tab as consumed always, so the gate used
  `wants_keyboard_input()` for keys rather than `response.consumed`.
- Since then: `wants_keyboard_input()` is true for any focused widget, so a
  focused button held the pads and kept keys from the game. Commit
  `ac25c88` (2026-10-28) made the gate ask whether the focused widget is a
  `TextEdit` (`text_field_focused`); the key gate, IME gate, pad hold and
  raw mouse motion follow it. [ADR 0110](0110-capture-and-focus-gates.md) added the pad hold and dropped raw
  mouse motion while a field has focus.

## Sources

- [#41](https://github.com/chrischaps/Flint/pull/41) / commit
  [`e8e355f`](https://github.com/chrischaps/Flint/commit/e8e355f48c13c8576dd15c1bae58232f632e6504)
  "[P1.3] H3: game egui layer with input routing (Flint)"
- Commit
  [`ac25c88`](https://github.com/chrischaps/Flint/commit/ac25c88e2916cc4cf42935542fab66a499ab404d)
  "Only a focused text field counts as typing"
- Vault: "2026-10-12 Capitol Flint Game UI Takes The Keyboard",
  "2026-10-11 Capitol Flint Spike Inscribes The Capitol"
- Code now: `crates/flint-player/src/player_app/egui_input.rs`,
  `crates/flint-player/src/player_app/events.rs`; book:
  [Input Gates](../book/src/concepts/input-gates.md),
  [Player Extensions](../book/src/guides/player-extensions.md), "Game UI
  with egui"
