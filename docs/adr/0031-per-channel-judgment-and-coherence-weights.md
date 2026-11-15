# ADR 0031: Per-channel judgment, with coherence weights inert by default (decision not recovered)

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commit `86d21ec` and the code it added.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Nothing cites ADR 0031 by number: no commit, comment, page or Vault note.
The only source is the commit that names the batch, "ADRs [0030](0030-full-verb-map-capture.md)-[0037](0037-manifest-map-timeline-panel.md)", and
lists its items in order: verb maps, "per-channel judgment and
inert-by-default coherence weights", degraded alternates, cues,
conducted-frame extensions, the two debug panels and quit-on-finish. The
items whose numbers are confirmed elsewhere (0030, 0032, 0033, 0035, 0036,
0037) follow that order, so this title is the second item. That mapping is
an inference.

## Decision

What the commit's code does, not what the record said:

- Judgment covers the new channels. A `Track` record names its channel
  (`lean`, `sway`, `pressure_l`, `pressure_r`); scalar channels use the x
  component. Press windows carry a depth error, `max(0, strength - peak
  pressure in the window)`, so over-squeezing is free. Flick windows carry
  the angle to the authored direction, 180 degrees when the gesture had
  none.
- In `Arrival` mode each channel's keys are arrival windows; `Track` mode
  stays lean-only.
- New log fields appear only on non-lean records, so lean-only charts and
  older logs stay byte-identical.
- `CoherenceConfig` gains `w_sway`, `w_pressure_l`, `w_pressure_r`,
  `press_depth_err_full` and `flick_dir_err_full_deg`. The weights default
  to 0, and a zero-weight non-lean record is skipped entirely, so an
  unmodified config gives bit-identical coherence on any chart.

The reasoning did not survive.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A World II tuning run found that adding channels changes how failure
  looks too: counter-bank sway targets and near-zero pressure curves score
  well at stick-neutral, giving coherence peaks of 0.85-0.92 while
  neglecting, so the full-fail exit had to move to 0.94 (Vault, "Star Child
  World II - The Undertow").

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Vault: "Star Child World II - The Undertow - 2026-08-17"
- Code now: `crates/flint-music/src/judgment.rs` (`JudgmentRecord`, `Judge`),
  `crates/flint-music/src/coherence.rs` (`CoherenceConfig`),
  `crates/flint-music/tests/world2_milestone.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Coherence"
