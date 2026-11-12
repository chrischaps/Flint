# ADR 0009: Sessions are recorded as compensated-sample events and replay headless

Status: accepted, 2026-08-15 (commit
[`f693ee9`](https://github.com/chrischaps/Flint/commit/f693ee93b6f2c5c1394ccffc114d2a73813912f9));
amended 2026-08-15 (commit
[`226ac74`](https://github.com/chrischaps/Flint/commit/226ac748d8e2ef9c99637dcab772a2f07fa112fc)).
Backfilled 2026-11-12 (#374) from those commits and `replay.rs`'s module doc.
The original record was in the Starchild repo, which no longer exists; this is
a reconstruction.

## Context

Coherence tuning is a primary goal of the feel prototype, and the raw judgment
logs exist so alternatives can be evaluated offline. That needs the player's
input as a file that the same judgment and coherence code can consume with no
controller, clock or audio.

## Decision

- **One file per session, `*.session.jsonl`.** Line 1 is a JSON header (suite,
  chart, sample rate, both timing offsets, the config snapshot, capture
  parameters); every further line is one event stamped with its compensated
  suite sample ([ADR 0008](0008-gamepad-capture-on-its-own-thread.md)).
- **Determinism is enforced at both ends.** The writer requires nondecreasing
  samples and change-compresses lean chatter; the reader re-checks
  monotonicity, so a hand-edited file can't silently break determinism.
- **Synthetic profiles** `perfect`, `late:<ms>` and `neglect` generate the
  same stream from a chart alone. The tests and the CLI share them.
- `flint play-chart --record` writes a session. `flint replay-chart` runs a
  recorded or synthetic session through the identical pipeline, with config
  precedence `--config` > the session's snapshot > the repo default, and an
  optional `--render` over the same span.

**Amendment (`226ac74`, reintegration):** a seam jumps the suite timeline
backwards ([ADR 0014](0014-reintegration-seam-on-one-clock-tick.md)), so
session files now record raw clock samples plus `timeline_jump` records, and
the reader rebuilds suite time from them. The `ClockBridge` is fed the raw
(monotonic) clock and remaps raw to suite at ingest. Sessions from before
Phase 3 have no jump records and read as raw = suite.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A live session and its replay produce the same judgment records; the
  commit verified this bit-identical across runs on the real CLI.
- Milestone evidence is a synthetic session run, not a controller session.
- Since then: the change compression applies to every continuous stream (lean,
  sway, both pressure triggers) through one `decimate` policy (commit
  [`97c48bb`](https://github.com/chrischaps/Flint/commit/97c48bbd4273ea897f02ea43755d55c6c3b0426f),
  2026-08-19 hygiene pass).

## Sources

- Commit [`f693ee9`](https://github.com/chrischaps/Flint/commit/f693ee93b6f2c5c1394ccffc114d2a73813912f9)
  "D8: session replay — record/replay format, synthesis, flint replay-chart"
- Commit [`226ac74`](https://github.com/chrischaps/Flint/commit/226ac748d8e2ef9c99637dcab772a2f07fa112fc)
  "E5 full-fail + reintegration sequencer"
- Code now: `crates/flint-music/src/replay.rs` (`SessionWriter`,
  `read_session_raw`, `synthesize`, `SyntheticProfile`),
  `crates/flint-cli/src/commands/replay_chart.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Recording,
  Replay and Offline Render"
