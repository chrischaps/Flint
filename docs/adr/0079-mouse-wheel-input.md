# ADR 0079: The mouse wheel reaches `InputState`, in lines

Status: accepted, 2026-10-16 ([#47](https://github.com/chrischaps/Flint/pull/47)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #47's description and
commit `761372a`.

## Context

The binding model and `InputState` were built for the wheel
(`Binding::MouseWheel`, `process_mouse_wheel`, the per-frame clear), but
`flint-player` dropped `WindowEvent::MouseWheel`. A
`type = "mouse_wheel"` binding validated and never fired, scripts couldn't
read the wheel, and an input script couldn't scroll. Every game that wanted
wheel zoom wrote its own extension hook, and only one consumer could read
it.

## Decision

The player feeds the wheel to `InputState` in lines, up and right positive,
as winit reports them. Touchpad pixel scroll is scaled at 50 px a line,
matching egui-winit, so a binding's `scale` means the same on a mouse and a
touchpad (`wheel_lines`). The wheel reaches `InputState` only when neither
egui nor an extension consumed the event, the same rule as clicks.
`InputState::mouse_wheel_delta()`; scripts get `mouse_wheel_x()` /
`mouse_wheel_y()` and input scripts a `wheel <dy> [dx]` verb, lasting one
frame like a real notch.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Opt-in: nothing changes for a game with no wheel binding and no script
  reading it. A game that already bound the wheel (none in-tree) sees it
  start working.
- The per-frame value is the sum of that frame's events, cleared in
  `end_frame`.
- Since then: [#81](https://github.com/chrischaps/Flint/pull/81) (no new
  ADR) made the input script's `wheel` a `PointerEvent::Wheel`, routed as a
  winit `MouseWheel` the way
  [ADR 0082](0082-input-script-mouse.md) routes the pointer. It no longer
  writes `InputState` itself, so a scripted wheel over a panel scrolls the
  panel.

## Sources

- [#47](https://github.com/chrischaps/Flint/pull/47) / commit
  [`761372a`](https://github.com/chrischaps/Flint/commit/761372a324afe526b9509dd447ebed88c84cb7ed)
  "[H11] Flint: the mouse wheel reaches actions, scripts and input
  scripts" (an earlier copy, `9f2b9d1`, is not on main)
- [#81](https://github.com/chrischaps/Flint/pull/81) / commit
  [`d987859`](https://github.com/chrischaps/Flint/commit/d987859e6c3587b6104350b824e3df42bdf3bebe)
  "[#299] Input scripts: the wheel goes through egui, as a real one does"
- Code now: `crates/flint-player/src/player_app/events.rs`
  (`wheel_lines`), `crates/flint-runtime/src/input.rs`
  (`InputState::mouse_wheel_delta`); book: [CLI overview](../book/src/cli-reference/overview.md) (input
  scripts)
