# ADR 0082: Input scripts play the mouse through egui's path

Status: accepted, 2026-10-17 ([#50](https://github.com/chrischaps/Flint/pull/50)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #50's description and
commit `73eaf84`.
Extended by [#81](https://github.com/chrischaps/Flint/pull/81) (the wheel,
no new ADR); keys took the same path in [ADR 0097](0097-input-script-keys-reach-egui.md).

## Context

`--input-script` wrote keys, pad and wheel straight into `InputState`. The
mouse matters to any game with UI. If scripted pointer events went straight
into `InputState` too, egui would never see them, and a scripted click
would fall through every panel into the world, the opposite of what a real
click does. The Capitol had tried posting `WM_MOUSEMOVE`/`WM_LBUTTONDOWN`
to the window by PID. That reached scripts but never egui, because winit's
`TrackMouseEvent` sends `WM_MOUSELEAVE` straight away while the real cursor
is elsewhere. It was also Windows-only, ran on wall-clock time, and broke
under `--frame-step`.

## Decision

- New verbs: `mouse <x> <y> [secs]` and `click <x> <y> [button]`. Mouse
  button names (`MouseLeft`, `MouseRight`, `MouseMiddle`, `MouseBack`,
  `MouseForward`) also work with `down`, `up` and `tap`. Positions are
  physical window pixels, the same pixels a screenshot has.
- `InputScript::apply` returns `Applied { keys, pointer }`. The player turns
  each `PointerEvent` into the `WindowEvent` a real mouse would send
  (`CursorMoved`, `MouseInput`). It routes them through
  `taken_before_the_game`, the egui gate and extension dispatch that
  `window_event` now shares, and then into `InputState`. Keys, pad and
  wheel are unchanged.
- Two things wait a frame, however coarse the frames:
  - A press in the frame the pointer jumped. egui-winit decides whether it
    wants a press from the hover state of the previous egui frame. Without
    the wait, a click on a panel the pointer had just reached would go to
    the world.
  - A release in the frame of its press, as keys already did.

  So a `click` takes three frames. A glide's per-frame steps don't hold a
  press back, so a drag can release mid-glide.
- A glide interpolates from wherever the script last put the pointer, and
  moves it once a frame until it arrives. A jump cancels a glide.

## Alternatives considered

- Writing pointer events straight into `InputState`, as keys were. Rejected
  for the reason in Context: egui would never see them.

## Consequences

- Deterministic captures of any egui interaction (drags, hover, tooltips,
  buttons) and of world clicks, cross-platform, with `--frame-step`.
- The real cursor still moves the pointer if it crosses the window
  mid-capture.
- `InputScript::apply`'s return type changed from `Vec<KeyCode>` to
  `Applied`. The only callers were the player and its tests.
- Text typing into egui fields was out of scope: keys went to `InputState`
  only. Since then: [ADR 0097](0097-input-script-keys-reach-egui.md) sends input-script keys through egui first,
  with a `type <text>` verb, and
  [#81](https://github.com/chrischaps/Flint/pull/81) does the same for the
  wheel (`PointerEvent::Wheel`), so a scripted wheel over a panel scrolls
  it.

## Sources

- [#50](https://github.com/chrischaps/Flint/pull/50) / commit
  [`73eaf84`](https://github.com/chrischaps/Flint/commit/73eaf849ae6796b44bc60d0c8078f989697a4de2)
  "input scripts play the mouse: pointer, buttons, drags, clicks"
- [#81](https://github.com/chrischaps/Flint/pull/81) / commit
  [`d987859`](https://github.com/chrischaps/Flint/commit/d987859e6c3587b6104350b824e3df42bdf3bebe)
  "[#299] Input scripts: the wheel goes through egui, as a real one does"
- Vault: "The Capitol - Input Scripts Play the Mouse (H16) - 2026-10-17",
  "2026-10-21 Capitol Scripted Wheel Scrolls Panels"
- Code now: `crates/flint-player/src/player_app/input_script.rs`,
  `crates/flint-player/src/player_app/events.rs`
  (`taken_before_the_game`, `scripted_pointer_event`); book:
  [CLI overview](../book/src/cli-reference/overview.md) (input scripts)
