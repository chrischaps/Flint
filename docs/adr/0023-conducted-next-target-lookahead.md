# ADR 0023: Scripts see the next authored lean key and the beats until it

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4`, Vault note "Starchild -
conducted_next_target needs spacing-aware blends (2026-08-17)", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The conducted frame ([ADR 0020](0020-conducted-parameters-script-surface.md)) gave scene scripts the chart's current lean
target. A binding that wants to move something toward where the chart bends
next had no way to see ahead. Why the lookahead was wanted, beyond that, did
not survive.

## Decision

From the commit:

- `ChartEval::next_key(channel, beat)`: the first authored key strictly
  after `beat`. A query exactly on a key returns the following key.
- `ConductedFrame` gains `next_target` and `next_target_beats`, for the
  `lean` channel.
- Rhai gets `conducted_next_target()`, a map `#{x, y, beats}`.
- The lookahead stays live during preroll: `pos.beat` is negative there, so
  `beats` counts down to the first key.
- Past the last key, `beats` is a 1e6 sentinel and `x, y` fall back to the
  current target.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- `next_target` flips to the following key exactly at each anchor. The
  Vault note found that a binding blending toward it over a fixed window
  (`clamp(1 - beats / WINDOW)`) is continuous only while key spacing is at
  least the window; with denser keys the bound object teleports part of the
  way at each flip. The prologue script works round it by measuring the
  spacing at each flip. The note suggests reporting the previous key or the
  spacing from the engine as the cleaner fix; the frame has neither today.
- With no session running the snapshot is neutral and the lookahead is
  effectively infinite.
- `crates/flint-music/tests/conducted_trace.rs` checks every frame's
  `next_target` and `next_target_beats`.

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "Starchild - conducted_next_target needs spacing-aware blends
  (2026-08-17)"
- Code now: `crates/flint-music/src/chart_eval.rs` (`ChartEval::next_key`),
  `crates/flint-music/src/chart_session.rs` (`ConductedFrame`),
  `crates/flint-script/src/api/conducted.rs` (`conducted_next_target`),
  [Scripting](../book/src/concepts/scripting.md),
  [Music Sessions](../book/src/concepts/music-sessions.md)
