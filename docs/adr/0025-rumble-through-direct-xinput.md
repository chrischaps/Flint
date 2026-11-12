# ADR 0025: Rhythm rumble writes the motors directly, bypassing gilrs's ff server

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4`, Vault note "2026-08-16
The 50ms Server Nobody Mentions", the module docs of
`crates/flint-input-capture/src/rumble.rs` and `flint-cli`'s `spike_rumble.rs`.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Star Child's haptics ([ADR 0026](0026-haptic-entrainment-architecture.md)) put a tick in the player's hands ahead of
each beat, to tighten pulse timing. The spike before it (H1) asked whether
force feedback fires on this stack at all, and which path from code to the
motors is precise enough. There were two:

- gilrs's `ff::Effect::play()` posts to a server thread that sleeps 50 ms
  per iteration (`gilrs::ff::TICK_DURATION`), so an effect starts on the
  next tick: a uniform 0-50 ms added latency.
- `gilrs_core::FfDevice::set_ff_state` is a direct, synchronous
  `XInputSetState`, with no queue.

A beat tick cannot carry 0-50 ms of jitter: the point was tightening a pulse
spread of about 36 ms.

## Decision

- Motors are written directly through `gilrs_core::FfDevice` from the 1 kHz
  capture thread in `flint-input-capture`, which already polls the pad. The
  gilrs ff server is not used for rhythm rumble.
- `flint spike-rumble` fires the motors, lets the operator feel the three
  weight prototypes (tick, thump, grind), times both paths and writes a
  report to `logs/latency/`.
- Actuator spin-up (about 20-50 ms for ERM motors) can't be measured in
  software on any path. A feel-tuned `lead_ms` absorbs it, seeded from
  measured latency minus about 30 ms.

## Alternatives considered

- **gilrs's high-level `ff::Effect`.** Rejected for the 50 ms server tick.

## Consequences

- The motor path is native only: a browser has no thread to spawn and no
  direct motor write, so on wasm32 capture and rumble report unavailable.
- `flint-music` never depends on gilrs; the decision layer ([ADR 0026](0026-haptic-entrainment-architecture.md)) emits
  events and the capture crate plays them.
- Since then: #748 added a general `rumble(pad, strength, seconds)` for any
  game, in `crates/flint-runtime/src/rumble.rs`. It goes through the ff
  server on purpose, rounded up to whole 50 ms ticks, which is fine for a
  hit and too coarse for a beat; while a music session runs the pads belong
  to the capture thread and that path isn't called. The pad wrapper moved
  to `flint-runtime` (`pad`) in the same change.

## Measured

- Direct `XInputSetState`: about 11 µs median (Vault note; the capture
  thread's comment says "~11 µs median — nowhere near the 1 ms poll
  budget").
- The 50 ms server tick is cited from gilrs 0.11 source, not measured; the
  spike's report says so.

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "2026-08-16 The 50ms Server Nobody Mentions"
- Code now: `crates/flint-input-capture/src/rumble.rs` (`spike_rumble`,
  `RumbleSpikeReport`), `crates/flint-input-capture/src/lib.rs` (the capture
  loop), `crates/flint-cli/src/commands/spike_rumble.rs`,
  `crates/flint-runtime/src/rumble.rs` (`PadRumble`),
  [Music Sessions](../book/src/concepts/music-sessions.md)
