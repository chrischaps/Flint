# ADR 0033: Chart cues fire into the conducted frame, carrying their flat `params` table

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commit `86d21ec`, Vault note "Star Child
World II - The Undertow - 2026-08-17", and the code comments citing it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A chart's `[[cues]]` (a `beat`, a `cue` name, an optional `params` table)
had been parsed and inert since an earlier milestone (B3). World II's scene
wanted authored moments it could bind to.

## Decision

- `resolve_cues` turns a chart's cues into samples against the conductor,
  sorted by sample then name; `ChartCore::set_cues` attaches them. A caller
  that never sets them gets a cue-free frame, the old behaviour.
- Each cue fires once, when judgment passes its sample, into
  `ConductedFrame::cues` for that frame, with `age_s` (suite seconds since
  its beat; normally under a frame).
- A rewind drops cues: one passed while rewinding is marked fired but not
  emitted. A seam re-arms every cue at or after the re-entry sample. The
  Vault note says this generalizes the prologue's rule that a rewind's
  backward motion is not an event.
- Scripts read them with `conducted_cues()`, an array of `#{name, age,
  params}`. `params` crosses into `flint-script` as a `CueParam` (`Number`,
  `Text`, `Flag`), so `flint-script` never depends on `flint-music` or toml
  shapes. Nested tables and arrays are dropped: authors write flat params.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The cue round-trip is pinned in `golden_fixtures.rs` and
  `world2_milestone.rs`.
- Cues are scene-facing only. Like the rest of the conducted surface they
  can't change the session.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Vault: "Star Child World II - The Undertow - 2026-08-17"
- Code now: `crates/flint-music/src/chart_session.rs` (`resolve_cues`,
  `ResolvedCue`, `ConductedCue`, `ChartCore::set_cues`),
  `crates/flint-script/src/context.rs` (`CueParam`, `ConductedCue`),
  `crates/flint-script/src/api/conducted.rs` (`conducted_cues`),
  `crates/flint-player/src/player_app/music_session.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md),
  [Scripting](../book/src/concepts/scripting.md)
