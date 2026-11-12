# ADR 0103: Extensions see an input script's keys through `on_key`

Status: accepted, 2026-10-21 ([#77](https://github.com/chrischaps/Flint/pull/77)).
Backfilled 2026-11-12 (#374) from PR #77 and commit `9fe56c5`. The PR body
describes the change but holds no separate ADR text, so this record is drawn
from the change itself. Follows [ADR 0097](0097-input-script-keys-reach-egui.md).

## Context

Since [ADR 0097](0097-input-script-keys-reach-egui.md), `--input-script` keys reach egui and `InputState`, but no
extension. `on_window_event` takes a winit `WindowEvent`, and a
`KeyboardInput` holds a `KeyEvent` whose `platform_specific` field is
`pub(crate)`, so nothing outside winit can build one. Anything a game read
off `KeyboardInput` in an extension couldn't be pressed by a capture
(TheCapitol#273, H29).

## Decision

`PlayerExtension::on_key(&KeyInput) -> bool`, default `false`, so existing
extensions are unchanged.

- `KeyInput` holds the `KeyCode`, state, repeat flag, the character the key
  stands for (winit's `logical_key`, on the release too; a scripted key reads
  a US layout), the modifiers held, and `scripted`. `pressed()` is a first
  press.
- A real key goes to each extension's `on_window_event` and then, if that
  passed, to the same extension's `on_key`, so the first extension to take a
  key still stops it.
- A scripted key goes to `on_key` only, after egui's focus gate and before
  the debug keys and `InputState`. A key an extension takes stays out of the
  game, as a real one would.
- Unidentified keys and winit's synthetic presses stay `on_window_event`'s.
- The player keeps the modifiers held (`key_modifiers`), from real and
  scripted `ModifiersChanged` alike.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Reading keys in `on_key` makes them scriptable. The book says to handle a
  key in one hook or the other.
- The Capitol opens its shortcut overlay with a scripted `?` and closes it
  with a scripted Esc; neither was possible before.
- Since then: during a capture, real window input is dropped and only the
  script's keys arrive (#91, an amendment to
  [ADR 0110](0110-capture-and-focus-gates.md)).

## Sources

- [#77](https://github.com/chrischaps/Flint/pull/77) / commit
  [`9fe56c5`](https://github.com/chrischaps/Flint/commit/9fe56c5ca1257c9230355939c29767a3740e22d1)
  "[H29] Flint: extensions see an input script's keys (ADR 0103)"
- Vault: "2026-10-21 Capitol Captures Can Press Question Mark"
- Code now: `crates/flint-player/src/extension.rs` (`on_key`, `KeyInput`),
  `crates/flint-player/src/player_app/events.rs`
- Book: [Player Extensions](../book/src/guides/player-extensions.md),
  [CLI overview](../book/src/cli-reference/overview.md)
