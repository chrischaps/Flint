# ADR 0026: Haptics are an event-shaped decision layer feeding a capture-thread rumble engine

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4`, Vault notes "2026-08-16
The 50ms Server Nobody Mentions" and "Starchild Programmers Guide Draft -
2026-08-17", and the module docs of `crates/flint-music/src/haptics.rs` and
`crates/flint-input-capture/src/rumble.rs`.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Star Child's design brief for touch was "entrainment, not guidance": the
pad's two motors are a light/heavy weight axis, not a direction, and the
pulse verb is purely temporal, so touch is used as a timekeeper. [ADR 0025](0025-rumble-through-direct-xinput.md)
settled how the motors are written. This record is the layer that decides
what to play and when.

## Decision

- `HapticsDriver` in `flint-music` is a pure decision layer. It maps
  musical structure to `HapticEvent`s: a pre-beat tick, a thump when a
  pulse lands, a grind during a rewind, ticks during the pickup.
- It is event-shaped, never state-shaped. It never sees lean error,
  coherence or any continuous wrongness, because a buzz-when-wrong reads as
  punishment. Its inputs are judged pulses, sequencer transitions and the
  grid.
- It touches no hardware. `RumbleEngine` in `flint-input-capture` runs on
  the 1 kHz capture thread and fires scheduled bursts at bridged-clock
  samples with about 1 ms resolution. Replay and offline renders attach no
  sink, so they are no-ops by construction.
- Bursts are addressed in suite samples; the caller maps them to raw clock
  samples on the way to the capture thread, which fires them early by
  `lead_ms`. A burst later than `late_drop_ms` past its fire point is
  dropped, not played: a late tick fights the ear and is worse than none.
- Thumps ride `HapticEvent::Immediate`, so a felt confirmation is never
  dropped.
- Configured by `config/haptics.toml`; with none, no event is emitted.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The commit fixed a clock-domain bug on the way: ear time to raw time
  needs the offset plus the output latency. The Vault's Programmer's Guide
  note calls the three clock domains (raw, suite, ear), with this bug as the
  example, the guide's most valuable section.
- It also fixed `Conductor::next_grid_sample`, which promised "strictly
  after" and broke at fractional beat lengths (below).
- The haptics config is loaded per session, not hot-reloaded; the hygiene
  pass on 2026-08-19 found that deliberate.
- The commit left the feel gate (H4) to Chris.

## Measured

- Every test tempo (120 and 90 BPM) divided 48 kHz evenly. The prototype's
  84 BPM is 34285.71 samples a beat, and rounding broke `next_grid_sample`'s
  promise on the first live run, as an 80 GB allocation abort in the new
  scheduling loop (Vault).

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "2026-08-16 The 50ms Server Nobody Mentions"
- Vault: "Starchild Programmers Guide Draft - 2026-08-17"
- Vault: "2026-08-19 Starchild hygiene pass — determinism as a refactoring
  tool"
- Code now: `crates/flint-music/src/haptics.rs` (`HapticsDriver`,
  `HapticsConfig`), `crates/flint-input-capture/src/rumble.rs`
  (`RumbleEngine`, `haptic_sink`), `crates/flint-music/tests/haptics_driver.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Audio Gradient
  and Haptics"
