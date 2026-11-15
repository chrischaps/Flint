# ADR 0013: Lean is judged by arrival at beat-anchored targets

Status: accepted, 2026-08-15 (commit
[`8a5fb9a`](https://github.com/chrischaps/Flint/commit/8a5fb9acbe2535c33d9815e8e57ca3097ed6d406)).
Backfilled 2026-11-12 (#374) from commit `8a5fb9a` and `judgment.rs`'s module
doc. The original record was in the Starchild repo, which no longer exists;
this is a reconstruction.

## Context

Lean was judged continuously (`LeanMode::Track`): on a 1/8-beat grid, the
distance between the stick and the chart's interpolated curve. Chris's first
hands-on feel check at the Milestone 2 [H] gate found continuous tracking at
a small radius too delicate to sustain flow.

## Decision

- A new `LeanMode::Arrival`: each lean key in the chart is a target to arrive
  at. Within ±0.3 beats of the key's beat (`arrival_half_beats`, "deliberately
  wide: arrival is gross motion, not aim") the best distance observed counts.
  The zero-order-hold latch counts, so rolling through the target on time is
  as good as parking on it. Motion between keys is free.
- It emits one record per key, the same shape as a `Track` record, so logs
  and downstream code are unchanged.
- The CLI defaults to `arrival`. `JudgmentConfig`'s default stays `track`, so
  the Milestone 2 automated evidence stays pinned.
- The prototype chart's lean was re-authored near the rim
  (`tools/gen_prototype_lean.py`, east-west rocking to compass laps).

## Alternatives considered

- **Continuous tracking (`Track`), the existing model.** Too delicate for
  flow at the first feel check. It stays available and is still the config
  default.

## Consequences

- Two lean modes, selected by `lean_mode`. The player's `music_session`
  component defaults to `arrival` too.
- Target and actual are logged raw in both modes, so either model can be
  re-evaluated offline from the logs.

## Measured

Synthetic perfect play on the re-authored chart: 92/92, coherence 1.00.

## Sources

- Commit [`8a5fb9a`](https://github.com/chrischaps/Flint/commit/8a5fb9acbe2535c33d9815e8e57ca3097ed6d406)
  "Arrival lean mode: beat-anchored targets rolled to in time (ADR 0013)"
- Code now: `crates/flint-music/src/judgment.rs` (`LeanMode`,
  `JudgmentConfig::arrival_half_beats`),
  `crates/flint-music/src/chart_session.rs` (`parse_lean_mode`),
  `crates/flint-player/src/player_app/music_session.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md)
