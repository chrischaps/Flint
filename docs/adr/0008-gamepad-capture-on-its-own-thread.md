# ADR 0008: Gamepad capture runs on its own 1 kHz thread, in a leaf crate

Status: accepted, 2026-08-15 (commit
[`762b6b0`](https://github.com/chrischaps/Flint/commit/762b6b0289e6649dc213b79a4817da87f75b73c7)).
Backfilled 2026-11-12 (#374) from commits `800b57f` and `762b6b0`, the module
docs of `flint-input-capture`, `input_stream.rs` and `clock_bridge.rs`, and the
Vault note "Starchild Phase 2 Planning - 2026-08-15". The original record was
in the Starchild repo, which no longer exists; this is a reconstruction.

## Context

Judgment needs each pulse's time in suite samples at millisecond grain. The
Phase 2 survey (Vault) found that `flint-player` discarded gilrs event
timestamps and its `InputState` was frame-quantized, with no vec2. Kira's
clock is readable only on the session thread and advances once per audio
callback, so a single paired read is quantized to the device buffer.

## Decision

- **A new leaf crate, `flint-input-capture`, owns gilrs.** A dedicated thread
  polls it, 1 kHz by default, far above frame rate.
- **Verb space, not buttons.** The thread maps the left stick to `lean`
  (radial deadzone) and South or the right trigger to `pulse`. Charts never
  see buttons.
- **Compensated stamps.** Each event is stamped through the `ClockBridge`
  (commit `800b57f`): a least-squares line through `(Instant, clock_sample)`
  pairs fed from the session loop, which averages the per-callback sawtooth
  and absorbs drift. The stamp is the bridged sample minus the judgment
  offset (output latency plus tap calibration), so it carries the musical
  moment the player responded to. Producers apply the offset; consumers never
  re-compensate.
- **The event types are gilrs-free** and live in `flint-music`
  (`input_stream.rs`), sent over an `mpsc` channel. The replay path builds the
  identical stream from a file
  ([ADR 0009](0009-session-replay-in-compensated-samples.md)).
- `measure_granularity()` histograms receipt against driver timestamp deltas
  rather than trusting the driver.

## Alternatives considered

Not recorded in the surviving text. The Vault records that player-app input
plumbing was deferred to Phase 4 on purpose, so Phase 2 stayed CLI-first.

## Consequences

- Judgment, coherence and logging never know whether input came from
  hardware or a file.
- `FlintError` gained `InputError`.
- Since then: the backend became XInput
  ([ADR 0011](0011-gilrs-on-the-xinput-backend.md)); the `full` verb map added
  `sway`, `pressure_l`/`pressure_r`, `press` and `flick` ([ADR 0030](0030-full-verb-map-capture.md)); the
  thread compiles out on wasm32 (#100) and without the `gamepad` feature
  (#221); ordinary pad input and script `rumble()` moved to
  `flint_runtime::pad`, leaving this crate the rhythm layer's alone (#748).

## Sources

- Commit [`800b57f`](https://github.com/chrischaps/Flint/commit/800b57fa74aac886f89e54968d25d22ae9828e1a)
  "D1a: clock bridge — Instant↔sample-clock regression with drift tracking"
- Commit [`762b6b0`](https://github.com/chrischaps/Flint/commit/762b6b0289e6649dc213b79a4817da87f75b73c7)
  "D1b/D2 core: flint-input-capture crate — 1 kHz verb-space capture +
  granularity spike"
- Vault: "Starchild Phase 2 Planning - 2026-08-15"; "Starchild Phase 2
  Implementation - 2026-08-15"
- Code now: `crates/flint-input-capture/src/lib.rs` (`VerbMap`,
  `measure_granularity`), `crates/flint-music/src/input_stream.rs`
  (`InputEvent`), `crates/flint-music/src/clock_bridge.rs` (`ClockBridge`),
  [Music Sessions](../book/src/concepts/music-sessions.md)
