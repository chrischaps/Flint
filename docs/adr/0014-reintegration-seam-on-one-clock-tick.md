# ADR 0014: A reintegration seam re-plays every stem on one clock tick

Status: accepted, 2026-08-15 (commit
[`89e6e10`](https://github.com/chrischaps/Flint/commit/89e6e108a92281448709175398312b516ea94bf4)).
Backfilled 2026-11-12 (#374) from commits `89e6e10` and `226ac74`,
`reintegration.rs`'s module doc, the book, and the Vault note "Starchild
Phase 3 - Where the Signal Actually Lives". The original record was in the
Starchild repo, which no longer exists; this is a reconstruction.

## Context

After a full fail the music has to return to a checkpoint: the previous
re-entry section. The Kira clock counts samples and can't rewind
([ADR 0005](0005-clock-counts-samples-tempo-map-speaks-music.md)), and the
stems must stay sample-locked across the return.

## Decision

"The seam is a timeline trick, not an audio trick" (Vault):

- **Retained stems.** `StemBus` keeps its `StaticSoundData`, so the mixer can
  `stop_all(fade)` and `replay_all_at` a position, every stem on one shared
  clock tick. This is Milestone 0's start mechanism reused.
- **The seam lands on a bar line.** Section starts sit on bar lines, so beat
  phase is continuous across the jump and the grid never breaks.
- **Raw and suite time are split.** The session keeps `suite = raw - offset`.
  `schedule_seam` commits a pending jump at the pump; the scheduler gains
  `flush()` and a `tick_base` (pre-roll plus offset). Judgment, coherence and
  replay see suite time and never notice the audio machinery; session files
  record the jumps
  ([ADR 0009](0009-session-replay-in-compensated-samples.md)).
- The `Conductor` gains `section_by_name` and `previous_re_entry`.

The sequencer built on this (`226ac74`): a debounced full-fail detector and a
`Playing → Failing → Reassembling → Playing` state machine. The lead bus
enters at full level, the others at -60 dB, ramping in over the manifest's
`reassembly_bars`, while the ladder runs in reverse. The judge is rewound at
the seam, and coherence is not reset.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- No hard cut and no interruption of input across a fail.
- Musical time jumps backwards at a seam, so ages measured in suite samples
  do too ([ADR 0020](0020-conducted-parameters-script-surface.md)).
- Since then: the seam fade became `[seam] fade_ms` (`e66cd10`); a rewind
  spin-down before the seam (`965e2b2`, measured in beats since `dcdb466`), a
  pickup spin-up into the re-entry downbeat (`975f0e4`) and `lead_in_beats`
  were added. The book lists them.

## Measured

From the spike (`tests/seam_spike.rs`): sample-locked re-entry under 1 µs
skew, no click, a single backwards jump, bit-identical renders.

## Sources

- Commit [`89e6e10`](https://github.com/chrischaps/Flint/commit/89e6e108a92281448709175398312b516ea94bf4)
  "E4 seam primitives: retained stems, replay-at-position, timeline offset
  (ADR 0014)"
- Commit [`226ac74`](https://github.com/chrischaps/Flint/commit/226ac748d8e2ef9c99637dcab772a2f07fa112fc)
  "E5 full-fail + reintegration sequencer"
- Commit [`e66cd10`](https://github.com/chrischaps/Flint/commit/e66cd10c6bebc4da14625669702a852358fe19b2),
  [`965e2b2`](https://github.com/chrischaps/Flint/commit/965e2b2169c712f9dc384b86ecdaa06d2ddd7a56),
  [`975f0e4`](https://github.com/chrischaps/Flint/commit/975f0e461d5666eed574bcc99bc2a1303c830c77),
  [`dcdb466`](https://github.com/chrischaps/Flint/commit/dcdb4668a72729326f7bf612460a173f3331281d)
- Vault: "Starchild Phase 3 - Where the Signal Actually Lives (2026-08-15)"
- Code now: `crates/flint-music/src/reintegration.rs` (`Reintegrator`,
  `SeqPhase`), `crates/flint-music/src/mixer.rs` (`StemBus`),
  `crates/flint-music/src/chart_session.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Full Fail and
  Reintegration"
