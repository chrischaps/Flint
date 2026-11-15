# ADR 0004: Stems are lossless; the validator refuses primed audio

Status: accepted, 2026-08-15 (commits
[`2c4d9c3`](https://github.com/chrischaps/Flint/commit/2c4d9c38ba70a422f9395ee3a26bc0081cde4065)
and
[`4db77a6`](https://github.com/chrischaps/Flint/commit/4db77a656b6a5f7dfd5aa1b57363ae1f63749879)).
Backfilled 2026-11-12 (#374) from those commits and the Vault note "Star Child
Milestone 0 - And the Validator Catches Its First Real Bug". The original
record was in the Starchild repo, which no longer exists; this is a
reconstruction.

## Context

The first `flint validate-suite` run on the placeholder track failed with
`StemDurationMismatch`: OGG encodes of three identical-length FLAC stems had
different frame counts. The cause, confirmed with a probe: libvorbis encoder
priming is per file (850 and 978 frames here), and symphonia, which Kira
decodes through, does not trim it. The stems would have been about 2.7 ms out
against each other and about 18 ms off the sample grid, silently.

## Decision

Stems are lossless only. The prototype plays FLAC directly, with Kira's
`flac` feature enabled. The probe reports a file's authored length and its
priming, and the validator errors `StemPrimingUnsafe` on any stem that
carries priming.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A suite's stems are FLAC or WAV. `validate.rs` tells an author to
  "re-export lossless (FLAC/WAV) for sample-locked stems".
- The workspace `kira` dependency still enables `ogg` and `mp3` for other
  sounds; the rule is the music validator's, not the audio engine's.

## Measured

Priming 850 and 978 frames on two stems; about 2.7 ms skew between stems and
about 18 ms against the grid. With FLAC, `flint play-suite` measured 0.000 ms
pairwise stem skew over 10 bars (Milestone 0).

## Sources

- Commit [`2c4d9c3`](https://github.com/chrischaps/Flint/commit/2c4d9c38ba70a422f9395ee3a26bc0081cde4065)
  "B5: placeholder track lands; manifest validates clean; OGG priming hazard
  found"
- Commit [`4db77a6`](https://github.com/chrischaps/Flint/commit/4db77a656b6a5f7dfd5aa1b57363ae1f63749879)
  "B6: flint play-suite — Milestone 0 passed"
- Vault: "Star Child Milestone 0 - And the Validator Catches Its First Real
  Bug" (2026-08-15)
- Code now: `crates/flint-music/src/validate.rs` (`StemPrimingUnsafe`),
  `crates/flint-music/src/probe.rs`
