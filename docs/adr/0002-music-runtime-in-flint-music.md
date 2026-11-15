# ADR 0002: The music runtime lives in a new `flint-music` crate

Status: accepted, 2026-08-14 (Starchild Phase 0; the crate first landed in
Flint in commit
[`5f94ff8`](https://github.com/chrischaps/Flint/commit/5f94ff84425153231f578961af29047617ec4409),
2026-08-15).
Backfilled 2026-11-12 (#374) from the Vault notes "Star Child Phase 0 - Spikes
Land, Latency Surprises" and "Star Child Phase 1 - The Clock That Refuses to
Speak Music". The original record was in the Starchild repo, which no longer
exists; this is a reconstruction.

## Context

With Kira chosen ([ADR 0001](0001-kira-is-the-music-backend.md)), the
conductor and the rest of Starchild's music machinery needed a home in the
engine, which the game carries as a subtree at `engine/`.

## Decision

What survives says two things:

- The conductor lives in a new engine crate, `flint-music`, not in
  `flint-audio` and not in the game.
- The record included a shared-`AudioManager` accessor, so music and the
  player's ordinary sounds could use one manager. The Phase 1 note says this
  accessor was "deliberately deferred to the Phase 3 boundary"
  ([ADR 0007](0007-suite-playback-keeps-its-own-manager.md)).

The reasons for a separate crate did not survive.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- `flint-music` holds manifests, charts, the tempo map and conductor,
  judgment, coherence, the ladder, reintegration, replay and offline render.
  It never touches a gamepad; `flint-input-capture` does
  ([ADR 0008](0008-gamepad-capture-on-its-own-thread.md)).
- Since then: the accessor landed in the F2 spike as
  `AudioEngine::manager_mut()` ([ADR 0017](0017-one-shared-audio-manager.md)).

## Sources

- Commit [`5f94ff8`](https://github.com/chrischaps/Flint/commit/5f94ff84425153231f578961af29047617ec4409)
  "B4: flint-music crate + flint validate-suite; golden-fixture tests pass"
- Vault: "Star Child Phase 0 - Spikes Land, Latency Surprises" (2026-08-14)
- Vault: "Star Child Phase 1 - The Clock That Refuses to Speak Music"
  (2026-08-15)
- Code now: `crates/flint-music/src/lib.rs`,
  `crates/flint-audio/src/engine.rs` (`AudioEngine::manager_mut`),
  [Music Sessions](../book/src/concepts/music-sessions.md)
