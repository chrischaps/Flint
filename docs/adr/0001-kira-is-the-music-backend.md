# ADR 0001: Kira is the backend for sample-locked music

Status: accepted, 2026-08-14 (a spike in the Starchild repo; no Flint commit).
Backfilled 2026-11-12 (#374) from the Vault note "Star Child Phase 0 - Spikes
Land, Latency Surprises" and a code comment in `crates/flint-music/src/mixer.rs`.
The original record was in the Starchild repo, which no longer exists; this is
a reconstruction.

## Context

Starchild's design needs stems that play sample-locked, with per-bus gain and
filtering changed on future beats. Phase 0 asked whether the audio library
Flint already linked could do that. `flint-audio` used Kira for spatial sound
but none of its clock machinery.

## Decision

Kira is the music backend. The spike (`spikes/audio_spike` in the game repo)
synthesized three stems in memory, scheduled them on one `ClockHandle` with
`StartTime::ClockTime`, ran per-bus gain and low-pass on sub-tracks, and queued
volume and filter events onto future beats before the clock started. The
Vault note records that all of this "was sitting unused in the dependency".

The record also noted a limit that code still cites: Kira has no per-track
pitch, so detune is per sound, through playback rate.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Music runs on the same Kira that `flint-audio` uses; the conductor went into
  a new crate ([ADR 0002](0002-music-runtime-in-flint-music.md)).
- `StemBus::set_detune` sets each sound's `PlaybackRate`. Any nonzero detune
  breaks sample-lock against the other stems while it lasts, so offline
  alignment asserts hold only at detune 0.

## Measured

Stem misalignment in the spike: 0.0000 ms.

## Sources

- Vault: "Star Child Phase 0 - Spikes Land, Latency Surprises" (2026-08-14)
- Code now: `crates/flint-music/src/mixer.rs` (`StemBus::set_detune`),
  `crates/flint-music/src/session.rs`
