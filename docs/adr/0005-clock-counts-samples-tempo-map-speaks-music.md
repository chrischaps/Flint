# ADR 0005: The clock counts samples; only the tempo map speaks musical time

Status: accepted, 2026-08-15 (commit
[`15b113c`](https://github.com/chrischaps/Flint/commit/15b113c1c6b6e68ef78e6b34e43bf7cf96c5c8aa)).
Backfilled 2026-11-12 (#374) from commits `c30141a` and `15b113c`, the module
docs of `session.rs` and `conductor.rs`, and the Vault note "Star Child Phase 1
- The Clock That Refuses to Speak Music". The original record was in the
Starchild repo, which no longer exists; this is a reconstruction.

## Context

The Milestone 0 notes and `playback.rs`'s module doc assumed tempo changes
would ride `ClockHandle::set_speed`, scheduled on tempo-map anchor
boundaries. The Vault note records why that was rejected: Kira advances
clocks once per internal chunk (128 frames), so a scheduled speed change
lands chunk-quantized. Each tempo change would add up to about 2.7 ms of
permanent, accumulating disagreement between clock-derived and map-derived
beats: two authorities for one quantity, drifting apart. Tempo is baked into
linear stems, so clock speed never affects the audio; `set_speed` would have
bought only readout convenience.

## Decision

- The Kira clock ticks once per sample (`TicksPerSecond(sample_rate)`) and
  never changes speed.
- `TempoMap`, through the `Conductor`, is the only thing that converts samples
  to beats and bars. Tempo and meter changes are arithmetic over anchors,
  snapped through the half-sample tolerance.
- The `Conductor` (commit `c30141a`) is the one query API for musical time. It
  holds no audio handles, never mutates, and its `now()` is pulled back by the
  measured output latency, so it answers for what the ear hears.

## Alternatives considered

- **`ClockHandle::set_speed` on anchor boundaries.** Rejected for the
  chunk-quantized drift described above.

## Consequences

- Clock-versus-tempo-map drift cannot happen: the clock never states musical
  time.
- Bar arithmetic across anchors lives in one place. The Vault note calls this
  "strictly less machinery" than the speed-change design.
- The scheduler resolves an event's musical time to a sample once, at push.

## Sources

- Commit [`c30141a`](https://github.com/chrischaps/Flint/commit/c30141a063cfebd56157b9a8cbd239c4eb54cc95)
  "C2: Conductor core — TempoMap inverses + bar arithmetic,
  latency-compensated query API"
- Commit [`15b113c`](https://github.com/chrischaps/Flint/commit/15b113c1c6b6e68ef78e6b34e43bf7cf96c5c8aa)
  "C3-C6: sample-tick clock, SuiteSession, six-bus mixer, lookahead
  scheduler, shared status readout"
- Vault: "Star Child Phase 1 - The Clock That Refuses to Speak Music"
  (2026-08-15)
- Code now: `crates/flint-music/src/session.rs`,
  `crates/flint-music/src/conductor.rs` (`Conductor`),
  `crates/flint-music/src/tempo.rs` (`TempoMap`),
  `crates/flint-music/src/scheduler.rs`
