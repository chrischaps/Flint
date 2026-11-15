# ADR 0064: A debug keyboard fallback for music sessions with no gamepad

Status: accepted, 2026-09-03 (commit
[`81e54fc`](https://github.com/chrischaps/Flint/commit/81e54fc196f95adeac7c64a0b5a6b69e503e0440)),
the date it reached Flint.
Backfilled 2026-11-12 (#374) from commit `81e54fc`, the book line that
names it and the code comments on its follow-up.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction. The decision was made in Starchild's engine subtree;
the date it was made there is not recorded.

## Context

A music session reads its verbs (`lean`, `pulse`) from a gamepad through
the 1 kHz capture thread, which owns the pad for the run ([ADR 0018](0018-gamepad-handoff-and-inputstate-downsample.md)). With no
pad connected the session played without input. The book calls the
fallback "a plumbing check, not a feel surface"; the surviving text says
nothing more about why.

## Decision

- In a `debug-hud` build, when no gamepad is visible at session start, the
  player arms keyboard input instead of spawning capture: arrow keys are
  `lean`, Space is `pulse`.
- The session still gets a live input receiver, so its NO INPUT diagnostics
  stay honest.
- The console says so loudly, and says it is prototype verbs only, "not a
  feel surface".
- Fully `debug-hud` gated; a release build always attaches the pad.

A follow-up, cited as "ADR 0064 follow-up" in the code: F9 forces the full
reintegration (rewind, seam, reassembly) through the ordinary trigger path,
so seam work doesn't depend on first playing down to a fail.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Commit `81e54fc` brought this upstream with four other Starchild engine
  features, all default-preserving; the workspace's 1107 tests passed.
- Since then: F9 with no session answers on screen (#367).

## Sources

- Commit
  [`81e54fc`](https://github.com/chrischaps/Flint/commit/81e54fc196f95adeac7c64a0b5a6b69e503e0440)
  "music/player/scene: upstream Starchild's five engine features"
- Code now: `crates/flint-player/src/player_app/music_session.rs`
  (`DebugKeyInput`, `debug_force_fail`),
  `crates/flint-player/src/player_app/events.rs`
- Book: [Music Sessions](../book/src/concepts/music-sessions.md)
