# ADR 0040: Config `schema_version` warns when missing; bad calibration files are named, not swallowed

Status: accepted, 2026-08-19 (commit
[`48e48b6`](https://github.com/chrischaps/Flint/commit/48e48b6ad80a6a2432c83f3e9e8eccf73f71d771)).
Backfilled 2026-11-12 (#374) from commit `48e48b6`, Vault note "2026-08-19
Starchild hygiene pass — determinism as a refactoring tool", and
`crates/flint-music/src/config_toml.rs`.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Star Child's pre-production hygiene pass (its records numbered 0038 to
0041) found error paths in `flint-music` that failed silently:

- `newest_toml` swallowed every failure with `.ok()?`, so a corrupt
  calibration or latency file read as "no record" and the session ran with
  a 0 ms judgment offset.
- Three casts saturated silently. A first-anchor bpm of 0 gave a preroll of
  `u64::MAX`, an unbounded wait.
- The four config formats (coherence, gradient, haptics, ladder) read a
  missing `schema_version` as 0 without a word, so an unversioned or
  misspelt key couldn't be traced.

## Decision

- A missing calibration directory stays the quiet no-record case. An
  unreadable or unparseable file is warned about by name and skipped, and
  the next-newest record is used.
- Checked conversions replace the saturating casts: `SuiteSession::start`
  errors on a non-positive or non-finite first-anchor bpm; the manifest's
  `audio.sample_rate` goes through `u32::try_from`; the offline render span
  through `usize::try_from`, with a cause instead of a panic at drain.
- Config `schema_version` moves from silent default to warn-if-missing:
  absent still parses as 0, so every committed config stays valid, but it
  warns and asks for `schema_version = 0`. A non-zero value is rejected.
- Content formats (chart, event script, suite manifest) keep requiring the
  key.

## Alternatives considered

- **Error if missing.** The commit says the record kept it as a
  propose-and-escalate follow-up for Chris. The Vault lists it as pending
  for him.

## Consequences

- The behaviour change is on error paths only; the A/B pin set stayed
  byte-identical (6/6).
- Since then: the parsing is shared in `config_toml.rs` as `VersionPolicy`
  (`Required` for content, `DefaultZeroStrict` for config). Its module doc
  says unifying the two policies would be a behaviour change to propose, not
  slip in. Config files still warn rather than error on a missing key.

## Sources

- Commit
  [`48e48b6`](https://github.com/chrischaps/Flint/commit/48e48b6ad80a6a2432c83f3e9e8eccf73f71d771)
  "Hygiene WS3.1+3+4: loud calibration errors; checked casts; version policy
  (ADR 0040)"
- Vault: "2026-08-19 Starchild hygiene pass — determinism as a refactoring
  tool"
- Code now: `crates/flint-music/src/config_toml.rs` (`VersionPolicy`,
  `parse_versioned`), `crates/flint-music/src/chart_session.rs`
  (`newest_toml`), `crates/flint-music/src/manifest.rs`
