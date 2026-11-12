# ADR 0010: Coherence is an asymmetric leaky integrator over bar time

Status: accepted, 2026-08-15 (commit
[`b10dc43`](https://github.com/chrischaps/Flint/commit/b10dc437e473d0334a63cdbd574d7a53786f4634)).
Backfilled 2026-11-12 (#374) from commit `b10dc43`, `coherence.rs`'s module
doc, the book and the Vault notes "Starchild Phase 2 Implementation -
2026-08-15" and "Starchild Phase 3 - Where the Signal Actually Lives". The
original record was in the Starchild repo, which no longer exists; this is a
reconstruction.

## Context

Judgment produces two raw error signals, lean tracking and pulse timing, and
no grades (`judgment.rs`). Everything downstream, the ladder, reintegration
and later the world's visuals, needs one value. The Phase 2 note puts the
intent this way: the scalar is "the world breathing, not a score".

## Decision

- Coherence is one value in [0, 1]. Each tracking record sets a target,
  `1 - w_lean * penalty`, and the value eases toward it with
  `alpha = 1 - exp(-Δbars/τ)`, rising with `rise_bars` and falling with
  `fall_bars`. The time constants are in bars, not seconds.
- Judged pulses enter as bounded impulses: a hit nudges up by how clean it
  was, a miss down by `miss_penalty`, a spurious pulse by `spurious_penalty`,
  which is 0 by default ("flow, not evaluation").
- Plain `f64` arithmetic in a fixed order: same records in, same value out.
- Every knob is in `CoherenceConfig`, loaded from `config/coherence.toml`.
  `play-chart` reloads it mid-session with the value carried over, and the
  session header and reload records snapshot the live config.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Tests pin the design's promises numerically, first in the commit, then in
  the Milestone 2 evidence (`tests/judgment_milestone2.rs`). The Phase 2 note
  records a test-design lesson: "coherence decays with neglect" is false from
  a cold start, so the test settles high with perfect play, then withdraws
  attention.
- The reachable band depends on the content as much as the integrator. In
  Phase 3, thresholds placed from the integrator's arithmetic (0.18, then
  0.35) were unreachable; the offline trace showed sustained neglect settling
  at 0.65-0.76. The ladder's thresholds were set from measurement instead
  ([ADR 0015](0015-disintegration-ladder-as-data.md)).
- Since then: `sway` and the pressure channels joined with weights that
  default to 0, so an unmodified config gives bit-identical values on any
  chart.

## Measured

From the commit's tests: rise above 0.9 by bar 4 under perfect play; smooth
decay at the configured τ; a single miss dips less than 0.1 and is erased
within two bars.

## Sources

- Commit [`b10dc43`](https://github.com/chrischaps/Flint/commit/b10dc437e473d0334a63cdbd574d7a53786f4634)
  "D5: coherence — asymmetric leaky integrator, live-tunable config,
  mid-session reload"
- Vault: "Starchild Phase 2 Implementation - 2026-08-15"; "Starchild Phase 3 -
  Where the Signal Actually Lives (2026-08-15)"
- Code now: `crates/flint-music/src/coherence.rs` (`Coherence`,
  `CoherenceConfig`), `crates/flint-music/tests/judgment_milestone2.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Coherence"
