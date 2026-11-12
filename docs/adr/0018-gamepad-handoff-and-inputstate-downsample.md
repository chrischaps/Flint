# ADR 0018: A session takes the gamepad and down-samples it into InputState

Status: accepted, 2026-08-16 (commit
[`187e756`](https://github.com/chrischaps/Flint/commit/187e75638922f99495c2912a42998f7cd6a9cb05);
built for real in
[`d95dc8b`](https://github.com/chrischaps/Flint/commit/d95dc8bab84aa5d76aeac229c91b653f061b0ef8)).
Backfilled 2026-11-12 (#374) from those commits, the code that cites it, the
book and the Vault note "Starchild F2 Player Integration Spike - 2026-08-16".
The original record was in the Starchild repo, which no longer exists; this is
a reconstruction.

## Context

Judgment needs the 1 kHz capture thread's compensated stamps
([ADR 0008](0008-gamepad-capture-on-its-own-thread.md)). The player polls
gilrs once a frame into `InputState`, and scripts read actions from that. Both
can't own the pad. The Vault note records that gilrs is not `Send`, so
"handing off" a gamepad between threads means the old owner drops its
instance and the new owner constructs its own.

## Decision

- **Handoff.** For a session's duration the capture thread owns the pad, and
  the player drops its own gilrs. The capture drain replaces the player's
  polling. When the session ends, the player's gilrs comes back.
- **One stream, two consumers.** `ChartSession::tick_with` hands each drained
  event to a tap: the judge consumes it at full precision, and the tap
  down-samples it into `InputState` on gamepad slot 0, so
  `is_action_pressed` and friends keep working, frame-quantized.
- **Pulse edges.** A pulse presses `South` and is released at the start of
  the *next* tick, so edge detection sees a full down/up pair.
- **Accepted gap.** Only `lean` (as the left stick) and `pulse` reach
  `InputState`; every other pad control is dropped inside the capture crate.
  Keyboard input stays on winit throughout. (Since the `full` verb map, the
  tap also drops `sway` and pressure events.)

## Alternatives considered

Not recorded in the surviving text. The Vault note says two live gilrs
instances on XInput were never tested, because drop-and-reconstruct worked
first try.

## Consequences

- Session scenes must not need any other pad control; the book says so.
- Since then: the `full` verb map's `sway` and pressure streams reach scripts
  through the conducted surface, not `InputState`
  ([ADR 0020](0020-conducted-parameters-script-surface.md)). A debug-hud
  build with no pad arms a keyboard fallback that feeds the same tap (ADR
  0064). Ordinary pad input and script `rumble()` moved to
  `flint_runtime::pad` (#748); starting a session stops that rumble and drops
  the player's gilrs.

## Measured

F2 live run `f2-1786900767`: 1557 events, 0 monotonicity violations, 12 of 13
pulses hit (mean |err| 15.9 ms); lean cadence was the known 8-16 ms XInput
batching.

## Sources

- Commit [`187e756`](https://github.com/chrischaps/Flint/commit/187e75638922f99495c2912a42998f7cd6a9cb05)
  "F2 PASSED: player integration spike — shared AudioManager + gilrs handoff
  (ADRs 0017, 0018)"
- Commit [`d95dc8b`](https://github.com/chrischaps/Flint/commit/d95dc8bab84aa5d76aeac229c91b653f061b0ef8)
  "F3: session-in-player lifecycle — music_session component ([ADR 0019](0019-music-session-component-in-the-player.md));
  spike deleted"
- Commit [`27be5c4`](https://github.com/chrischaps/Flint/commit/27be5c4fe45b0485402a384841aa98896f3e8fc2)
  "refactor(player_app): decompose tick into named steps"
- Vault: "Starchild F2 Player Integration Spike - 2026-08-16"
- Code now: `crates/flint-player/src/player_app/music_session.rs`
  (`MusicSession::tick`), `crates/flint-player/src/player_app/frame.rs`
  (`tick_music_session`), `crates/flint-music/src/chart_session.rs`
  (`tick_with`), [Music Sessions](../book/src/concepts/music-sessions.md),
  "Lifecycle"
