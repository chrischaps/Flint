# ADR 0024: Lean error drives a pitch wobble, not a detune, through the one mixer writer

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4`, Vault note "2026-08-16
Audio Gradient - Tuning by Wobble Not Offset", the module doc of
`crates/flint-music/src/gradient.rs`, and the book.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The disintegration ladder ([ADR 0015](0015-disintegration-ladder-as-data.md)) degrades the music in steps as
coherence falls. Star Child also wanted a continuous signal the player can
climb: how far the lean is from the chart's target, heard directly. The
prologue scene already turned the same error into fog haze, and the gradient
is described as that effect's audio twin.

## Decision

- A `GradientDriver` in `flint-music` maps two numbers onto the mixer:
  - **Tune:** lean error sets the *depth* of a zero-mean pitch LFO on one
    designated, degradable bus. Off the lean the voice wavers; on it, it
    settles. Optionally the error also thins that bus a little.
  - **Sink:** at stick-neutral, per-bus gain trims thin the mix, scaled by a
    smoothed neutral amount.
- The driver is a pure evaluator. It keeps its own slew state and returns
  `GradientOffsets`, which are summed into `LadderDriver`'s targets, so the
  mixer still has one writer.
- It never goes through Rhai: scripts are read-only toward audio.
- It is configured by `config/gradient.toml` and inert without it.
- Motif buses and the foundation stay in tune whatever the error.

Why depth and not a flat offset, from the module doc and the Vault note:
kira's only pitch control is playback rate, so a sustained one-sided detune
is a clock. It drifts the stem against the locked timeline with no seam to
re-lock it. A zero-mean LFO says "out of tune" to the ear while its drift
stays bounded, the precedent being the ladder's warble.

## Alternatives considered

- **A flat detune proportional to the error.** Rejected for the drift above.

## Consequences

- On the placeholder track `world_voice` was silent, so the device had
  nothing to act on. The Vault note says it was made to warn at startup
  rather than be found in a feel session.
- The commit left loudness tuning to Chris; the retrospective that day still
  listed "gradient loudness" as an open human gate.
- Since then: the config path follows the session's shared contract (an
  explicit `gradient_config` must load; the default file is optional;
  absent is inert), and `flint replay-chart` takes the same config.

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "2026-08-16 Audio Gradient - Tuning by Wobble Not Offset"
- Vault: "Starchild Prototype Retrospective - 2026-08-17"
- Code now: `crates/flint-music/src/gradient.rs` (`GradientDriver`,
  `TuneConfig`, `GradientOffsets`), `crates/flint-music/src/ladder.rs`
  (`LadderDriver::apply`), `crates/flint-music/tests/gradient_driver.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Audio Gradient
  and Haptics"
