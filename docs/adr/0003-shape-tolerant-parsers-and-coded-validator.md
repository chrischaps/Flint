# ADR 0003: Suite files parse shape-tolerantly; a validator reports coded issues

Status: accepted, 2026-08-15 (commit
[`5f94ff8`](https://github.com/chrischaps/Flint/commit/5f94ff84425153231f578961af29047617ec4409)).
Backfilled 2026-11-12 (#374) from commit `5f94ff8`, the module docs of
`flint-music`, and the Vault note "Star Child Milestone 0 - And the Validator
Catches Its First Real Bug". The original record was in the Starchild repo,
which no longer exists; this is a reconstruction.

## Context

A Starchild suite is two TOML files: a manifest (tempo map, sections,
re-entry points, a fixed six-bus stem inventory) and a beatmap chart (input
curves, pulses, cues). Schema v0 was pinned by Chris with golden fixtures in
the game repo before the validator was written; the production plan's words,
quoted in the Vault, were "build the validator before there is much to
validate".

## Decision

Commit `5f94ff8` (task B4) created `flint-music` with:

- **Shape-tolerant parsers.** `SuiteManifest` and `Chart` accept any bus
  names, section order, channels or version number. Only structurally
  unreadable input (missing tables, wrong types) fails the parse.
- **A validator with stable issue codes.** Structural and asset passes report
  semantic problems as `Issue`s with string codes. The game repo's
  `fixtures/invalid/*` files each carry a `# expect-error: <Code>` header, so
  renaming a code is a fixture change. When the code and the fixtures
  disagree, the fixtures are correct.
- **`TempoMap`**, sample position to beats and bars across tempo and meter
  changes, "the seed of the Conductor".
- **`flint validate-suite`**, exiting 1 on any issue.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The validator caught a real bug on first contact with real assets: OGG
  stems of equal-length FLACs decoded to different frame counts
  ([ADR 0004](0004-lossless-stems-only.md)).
- `TempoMap`'s half-sample bar-line tolerance let 84 BPM at 48 kHz (bars on
  fractional samples) use rounded section starts.
- Since then: the golden fixtures lived in the Starchild repo, which no longer
  exists. `tests/golden_fixtures.rs` and `tests/chart_eval_golden.rs` look for
  `../../../fixtures` and skip silently when it is absent, so in a Flint
  checkout the pinned codes are not tested.

## Sources

- Commit [`5f94ff8`](https://github.com/chrischaps/Flint/commit/5f94ff84425153231f578961af29047617ec4409)
  "B4: flint-music crate + flint validate-suite; golden-fixture tests pass"
- Vault: "Star Child Milestone 0 - And the Validator Catches Its First Real
  Bug" (2026-08-15)
- Code now: `crates/flint-music/src/manifest.rs`,
  `crates/flint-music/src/validate.rs` (`Issue`),
  `crates/flint-music/src/tempo.rs` (`TempoMap`),
  `crates/flint-music/tests/common/mod.rs` (`fixtures_dir`),
  [Music Sessions](../book/src/concepts/music-sessions.md)
