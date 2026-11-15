# ADR 0097: Input scripts' keys reach egui as a real keyboard's would

Status: accepted, 2026-10-20 ([#70](https://github.com/chrischaps/Flint/pull/70)).
Backfilled 2026-11-12 (#374) from PR #70, whose body holds the ADR text,
commit `33703d8`, and the Vault note "The Capitol - Input Scripts Type into
egui (H27, GH-189)".

## Context

`--input-script` wrote keys straight into `InputState`. Only the pointer took
winit's path ([ADR 0082](0082-input-script-mouse.md)), so egui never saw a scripted key: no `Key` event, no
text, no modifiers. A capture couldn't shift-drag between egui widgets, press
Enter or Escape in a field, or type (TheCapitol#189, H27).

## Decision

- `InputScript::apply` returns one ordered list of `ScriptEvent`s: `Pointer`,
  `Key(KeyCode, ElementState)`, `Modifiers(ModifiersState)` and
  `Text(String)`. Keys no longer write `InputState` themselves.
- Shift, Control, Alt and Super keep a held set in the script. When a press
  or release changes it, a `Modifiers` event comes just before that key's
  own. The player sends it as a real `WindowEvent::ModifiersChanged` through
  egui and extensions, so egui-winit's modifiers are right for the clicks
  and drags that follow.
- A key reaches egui as the events egui-winit would have made of it
  (`egui_input::scripted_key_events`): the `Key`, and on a press without
  Ctrl or the command key, the `Text` it types on a US layout. Then the same
  `EguiInputGate` as a real key: while egui wants the keyboard the key stops
  there. Otherwise it goes to the debug keys (H9) and `InputState`. A
  scripted Esc doesn't quit.
- A new verb, `type <text>`, sends the rest of its line as one egui `Text`
  event, without pressing keys.

## Alternatives considered

- **Send winit `KeyboardInput` through the pointer's route.** It can't be
  built: `winit::event::KeyEvent` has a `pub(crate)` platform field, so
  nothing outside winit can construct one.

## Consequences

- A key typed into a focused field no longer reaches `InputState` or the
  debug keys, as with a real keyboard. A behaviour change only for scripts
  that typed while an egui field had focus.
- Extensions' `on_window_event` never sees a scripted key; modifier changes
  do reach them. Since then:
  [ADR 0103](0103-extensions-see-scripted-keys.md) adds
  `PlayerExtension::on_key`, which does.
- No cut, copy or paste from scripted Ctrl+X/C/V, since they need the
  clipboard. Text is a US layout's.
- Since then: the gate counts only a focused text field, not any focused
  widget (#88, an amendment to
  [ADR 0110](0110-capture-and-focus-gates.md)); scripted keys take it too.

## Sources

- [#70](https://github.com/chrischaps/Flint/pull/70) / commit
  [`33703d8`](https://github.com/chrischaps/Flint/commit/33703d84e96503460d41afc6428163c9a2857d37)
  "[H27] Input scripts' keys reach egui (ADR 0097)"
- [#88](https://github.com/chrischaps/Flint/pull/88) / commit
  [`ac25c88`](https://github.com/chrischaps/Flint/commit/ac25c88e2916cc4cf42935542fab66a499ab404d)
  "Only a focused text field counts as typing"
- Vault: "The Capitol - Input Scripts Type into egui (H27, GH-189) - 2026-10-20"
- Code now: `crates/flint-player/src/player_app/input_script.rs`
  (`ScriptEvent`), `crates/flint-player/src/player_app/egui_input.rs`,
  `crates/flint-player/src/player_app/events.rs`
- Book: [CLI overview](../book/src/cli-reference/overview.md), input scripts
