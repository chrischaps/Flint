# ADR 0110: Capture and focus gates decide who hears the mouse and the pads

Status: accepted, 2026-10-21 ([#84](https://github.com/chrischaps/Flint/pull/84)).
Backfilled 2026-11-12 (#374) from PR #84, whose body holds the ADR text,
commit `adcab47`, the amendments in PRs #88 and #91, and the Vault notes "The
Capitol - capture and focus gates (GH-320, ADR 0110)" and "The Capitol - who
hears the input, three Flint gate fixes (GH-516, GH-513, GH-576)".
Amended by #88 and #91 (below).

## Context

Mouse-look and pad play need the player to be strict about who hears what
(TheCapitol#320, T1.2 of its controls plan). On Windows a captured cursor is
confined, not locked: it keeps moving while hidden, egui still gets its moves
and clicks, so an invisible click can press a button, and `mouse_x()`
follows it. Pads and raw mouse motion skipped the text-focus gate ([ADR 0073](0073-game-egui-layer-and-input-routing.md)),
and pads were read while the window was unfocused. Scripts couldn't tell
whether the cursor was captured: `set_cursor_captured` is a one-shot request,
and F3/F4 and Escape drop it. Input scripts couldn't make a raw mouse delta.

## Decision

1. **A captured cursor reads as the window centre.** `mouse_x()`/`mouse_y()`
   stay at the centre and follow a resize. Its `CursorMoved` reaches no one.
   Its buttons and wheel skip egui but reach extensions and the game, except
   a release whose press egui took before the capture
   (`EguiInputGate::skips_captured`). egui gets `PointerGone` on capture;
   release puts the OS cursor at the centre. Capture works without a window,
   as state only.
2. **A getter.** `is_cursor_captured()` for scripts,
   `ExtensionCtx::is_cursor_captured()` and `PlayerApp::is_cursor_captured()`.
   A request shows from the next frame.
3. **The text-focus gate covers pads and raw motion.**
   `InputState::hold_gamepads(bool)`: while held, sticks and triggers read 0
   and buttons up. A button held when the hold began reads up until released,
   so it never fires a press nobody made. The player holds the pads while an
   egui field has keyboard focus, and drops raw `MouseMotion` then too.
4. **The window's focus.** While unfocused, real (gilrs) pad events are
   dropped and raw motion ignored. Losing focus releases every slot a real
   pad wrote; regaining it reads the sticks again. The input script's pad
   isn't gated by focus, because captures often run in the background.
5. **Input scripts: `look dx dy [secs]`** feeds raw motion, at once or spread
   over game time, and takes a real mouse's gates but not the window's focus.

Nothing needs turning on; nothing changes for a game that never captures the
cursor or binds a pad.

### Amendments

- **Only a text field counts** (#88, 2026-10-28, for TheCapitol#516). egui's
  `wants_keyboard_input()` is true for any focused widget, so a focused
  button, tab or slot held the pads. The gate now asks whether egui's focused
  widget keeps a `TextEditState` (`egui_input::text_field_focused`). The key
  gate, the IME gate, the pad hold and raw motion all follow it; a focused
  button still sees its keys, and the game hears them too.
- **A capture hears its script alone** (#91, 2026-10-28, for
  TheCapitol#576). While a run shoots, stops on its own or plays an input
  script (`CaptureConfig::is_capture()`), real window input, raw motion and
  real pads are dropped; the script's events keep every gate. Lifecycle
  events (focus, resize, close) still arrive.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A game can do mouse-look over egui panels without stray clicks, and type
  into a field without the pad walking.
- A button held across a focus change needs a second press, the safe side.
- The music session's own pad capture ([ADR 0018](0018-gamepad-handoff-and-inputstate-downsample.md)) is untouched.
- With #91, a capture window under someone's cursor no longer takes a stray
  scroll as camera input.

## Measured

From #91, The Capitol in release with real wheel notches sent over the
capture window every 250 ms: on main, 642,318 of 921,600 pixels (69.7%) off
against a clean run; with the change, 6 and 0 pixels off in two runs.

## Sources

- [#84](https://github.com/chrischaps/Flint/pull/84) / commit
  [`adcab47`](https://github.com/chrischaps/Flint/commit/adcab478ef55b31c7cfec014ca2212fc8d3fad98)
  "Capture and focus gates for mouse-look and pads (ADR 0110)"
- [#88](https://github.com/chrischaps/Flint/pull/88) / commit
  [`ac25c88`](https://github.com/chrischaps/Flint/commit/ac25c88e2916cc4cf42935542fab66a499ab404d)
  "Only a focused text field counts as typing"
- [#91](https://github.com/chrischaps/Flint/pull/91) / commit
  [`1959e35`](https://github.com/chrischaps/Flint/commit/1959e355ce5b434cd2b99bd99d58a37bd4750f01)
  "A capture hears its input script alone"
- Vault: "The Capitol - capture and focus gates (GH-320, ADR 0110) - 2026-10-21",
  "The Capitol - who hears the input, three Flint gate fixes (GH-516, GH-513,
  GH-576) - 2026-10-28"
- Code now: `crates/flint-player/src/player_app/events.rs`,
  `crates/flint-player/src/player_app/egui_input.rs` (`text_field_focused`),
  `crates/flint-player/src/player_app/mod.rs` (`is_capture`),
  `crates/flint-runtime/src/input.rs` (`hold_gamepads`),
  `crates/flint-player/src/player_app/capture_gate_tests.rs`
- Book: [Cursor Capture and Input Gates](../book/src/concepts/input-gates.md)
