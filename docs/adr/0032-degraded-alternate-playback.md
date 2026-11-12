# ADR 0032: A composed degraded alternate plays as a muted, sample-locked twin of its bus

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commit `86d21ec`, Vault notes "Star Child
World II - The Undertow - 2026-08-17" and "2026-08-17 Musicians Guide drafted
(Starchild)", and the code comments citing it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The ladder ([ADR 0015](0015-disintegration-ladder-as-data.md)) degrades a bus with filters, trims and detune. A suite
can also carry a pre-composed degraded take of a bus, named in the manifest's
`[[degraded_alternates]]` over a sample range. Until World II nothing played
them; the Musicians Guide had called them "unexercised".

## Decision

- An alternate is a full-length stem, authored silent outside its span,
  started muted at the same shared clock tick as every other stem. It is
  sample-locked by construction.
- It rides its bus's own track, so the ladder's low-pass and trim reach both
  halves of the crossfade. The crossfade is per-sound volume (0 = bus,
  1 = alternate).
- A ladder rung lists the buses whose alternate takes over at that rung
  (`RungAudio::alternates`). Only degradable buses qualify, and only where
  the manifest authored one; elsewhere it is a silent no-op. The crossfade
  length is `[alternates] xfade_ms` in the ladder config.
- The crossfade is gated on the span against the *effective* mix, so a held
  rung crossing a span edge re-issues instead of leaving the bus ducked
  against authored silence.
- At a seam the alternate re-plays from the same tick, muted, and the
  crossfade resets to the bus: the world re-enters clean. During
  reassembly the per-sound volume belongs to the entry envelopes, so
  `LadderDriver::apply` is passed no alternates there.
- `StemResolver::load_alternate` defaults to `Ok(None)`, so resolvers that
  provide no alternates stay one method and the session warns.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- World II plays two alternates (Musicians Guide note). The feel gate was
  still open.
- `world2_milestone.rs` checks alternates with synthetic tone stems, never
  the game's audio.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Vault: "Star Child World II - The Undertow - 2026-08-17"
- Vault: "2026-08-17 Musicians Guide drafted (Starchild)"
- Code now: `crates/flint-music/src/mixer.rs` (`AlternateVoice`,
  `BusMixer::attach_alternate`), `crates/flint-music/src/ladder.rs`
  (`RungAudio::alternates`, `LadderDriver::apply`),
  `crates/flint-music/src/session.rs` (`StemResolver::load_alternate`),
  `crates/flint-music/src/reintegration.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Suite Manifest
  and Chart"
