# ADR 0007: Live suite playback keeps its own AudioManager until Phase 3 (partly recovered)

Status: accepted, 2026-08-15 (Starchild Phase 1; nearest Flint commit
[`15b113c`](https://github.com/chrischaps/Flint/commit/15b113c1c6b6e68ef78e6b34e43bf7cf96c5c8aa)).
Backfilled 2026-11-12 (#374) from one line of the Vault note "Star Child Phase
1 - The Clock That Refuses to Speak Music" and commit `15b113c`. The original
record was in the Starchild repo, which no longer exists; this is a
reconstruction. Superseded in effect by
[ADR 0017](0017-one-shared-audio-manager.md).

## Context

[ADR 0002](0002-music-runtime-in-flint-music.md) called for an accessor that
would let music share `flint-audio`'s Kira `AudioManager`. In Phase 1 the only
hosts were CLI harnesses, with no player and no other sounds.

## Decision

All that survives is one line: "[ADR 0002](0002-music-runtime-in-flint-music.md)'s shared-AudioManager accessor
deliberately deferred to the Phase 3 boundary (ADR 0007); live playback still
opens its own manager." Phase 1's ADRs were 0005-0007, so this is a Phase 1
decision. Its title, its reasons and anything else it decided did not
survive.

What the code of that day shows: `SuiteSession::start` is generic over the
backend and runs on a caller-owned `AudioManager`, and `RealtimePlayer` keeps
the device path, opening its own manager.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The CLI harnesses (`play-suite`, `play-chart`, `calibrate`) open their own
  manager to this day.
- Since then: the accessor arrived later than "the Phase 3 boundary", in
  Phase 4's F2 spike, as `AudioEngine::manager_mut()`, and the player's
  session opens on that one manager
  ([ADR 0017](0017-one-shared-audio-manager.md)).

## Sources

- Commit [`15b113c`](https://github.com/chrischaps/Flint/commit/15b113c1c6b6e68ef78e6b34e43bf7cf96c5c8aa)
  "C3-C6: sample-tick clock, SuiteSession, six-bus mixer, lookahead
  scheduler, shared status readout"
- Vault: "Star Child Phase 1 - The Clock That Refuses to Speak Music"
  (2026-08-15)
- Code now: `crates/flint-music/src/session.rs` (`SuiteSession`,
  `RealtimePlayer`)
