# ADR 0006: A session renders offline, deterministically, through the live code

Status: accepted, 2026-08-15 (commit
[`9814fca`](https://github.com/chrischaps/Flint/commit/9814fcab2e0290abb5bce19988f95f1d0dffd32a));
amended 2026-08-15 (commit
[`e66cd10`](https://github.com/chrischaps/Flint/commit/e66cd10c6bebc4da14625669702a852358fe19b2)).
Backfilled 2026-11-12 (#374) from those commits, `offline.rs`'s module doc and
the Vault note "Star Child Phase 1 - The Clock That Refuses to Speak Music".
The original record was in the Starchild repo, which no longer exists; this is
a reconstruction. No surviving text gives ADR 0006's title. It is one of the
Phase 1 records (ADRs [0005](0005-clock-counts-samples-tempo-map-speaks-music.md)-[0007](0007-suite-playback-keeps-its-own-manager.md), per the Vault), and the code cites it for "the
deterministic offline renderer"; this record is built on that.

## Context

Phase 1's evidence had to be something an agent could run: a scripted bus
drop landing on a bar line after a tempo change, with no clicks, repeatable.
A listening session can't assert that.

## Decision

- `OfflineBackend` is a custom Kira backend that keeps the `Renderer` and is
  driven through `AudioManager::backend_mut`: one thread, no device, no wall
  clock. All time flows from `process()` in fixed chunks, so two renders of
  the same inputs are bit-identical and faster than realtime. Pre-roll is
  rendered and dropped, so output index = suite sample.
- It drives the same `SuiteSession` that plays live (`SuiteSession` is generic
  over the backend, with a `StemResolver`).
- Event scripts (`*.events.toml`, versioned) put bus changes and markers at
  `bar:N[:B]`, `beat:F` or `sample:N`. `flint render-suite` renders one to a
  32-bit float WAV.
- `analysis.rs` adds "programmatic ears" over the buffer: Goertzel tone power,
  envelopes, a transition locator and a click detector.

**Amendment (`e66cd10`):** the record was amended for "director + reactive
replay". `render_offline_with` takes a director that mutates the session per
chunk (added in `89e6e10`), and `replay-chart --render` runs the full
reactive loop inside the offline render when a ladder config is present.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Milestone 1 passed on a test (`tests/offline_milestone1.rs`), not a
  listening session. Phase 3's threshold findings were "a 9-second test run,
  not a controller session" (Vault, Phase 3).
- Kira advances clocks per chunk, so events land with chunk granularity;
  tight asserts render with 128 frames or less.
- The Vault note records a trap: Kira's `MockBackend` keeps its frames
  private, hence the custom backend.

## Measured

Milestone 1 (`c538321`): a scripted drop at bar 9 landed at sample 768000
exactly; a restore after a 120 to 90 BPM, 4/4 to 3/4 change landed 448 samples
into its 480-sample ramp; two renders bit-identical.

## Sources

- Commit [`9814fca`](https://github.com/chrischaps/Flint/commit/9814fcab2e0290abb5bce19988f95f1d0dffd32a)
  "C4/C7: offline render harness, event-script TOML v0, flint render-suite"
- Commit [`c538321`](https://github.com/chrischaps/Flint/commit/c5383215a2ffc0c8b2c3e13991f8de39bfa5b794)
  "C8: analysis asserts + automated Milestone 1 evidence — MILESTONE 1
  PASSED"
- Commit [`e66cd10`](https://github.com/chrischaps/Flint/commit/e66cd10c6bebc4da14625669702a852358fe19b2)
  "E6+E7: reactive replay render, fail-timing sweep, tunable seam fade;
  status update"
- Vault: "Star Child Phase 1 - The Clock That Refuses to Speak Music"
  (2026-08-15); "Starchild Phase 3 - Where the Signal Actually Lives"
  (2026-08-15)
- Code now: `crates/flint-music/src/offline.rs` (`render_offline`,
  `render_offline_with`), `crates/flint-music/src/analysis.rs`,
  `crates/flint-cli/src/commands/render_suite.rs`,
  `crates/flint-cli/src/commands/replay_chart.rs`,
  [Music CLI](../book/src/cli-reference/music.md)
