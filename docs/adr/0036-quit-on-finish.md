# ADR 0036: `quit_on_finish` exits the player when a music session ends on its own

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commit `86d21ec`, the book and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

When a session finishes mid-scene, the player hands the pads back and the
scene keeps running in the post-session neutral world. The commit lists
"quit-on-finish" among its items and the book cites this number for the
field. Why it was wanted did not survive.

## Decision

- The `music_session` component takes `quit_on_finish` (bool, default
  `false`).
- When the session ends on its own and the field is true, the player stops
  the session and exits, printing `[music] quit_on_finish — exiting player`.
- Otherwise the scene keeps running, as before.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Only a natural finish counts: the check is on the session-finished path in
  `frame.rs`.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Code now: `crates/flint-player/src/player_app/music_session.rs`
  (`MusicSession::quit_on_finish`), `crates/flint-player/src/player_app/frame.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "The
  `music_session` Component"
