# ADR 0039: Script and clock-bridge locks recover from poisoning instead of panicking

Status: accepted, 2026-08-19 (commit
[`993118d`](https://github.com/chrischaps/Flint/commit/993118d5796b57a78180b9faa585b8c9c0b15066)).
Backfilled 2026-11-12 (#374) from commit `993118d`, Vault note "2026-08-19
Starchild hygiene pass — determinism as a refactoring tool", and the doc
comments on the two `lock_or_recover` helpers.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

`flint-script` has a log-and-continue policy: a failing script logs and the
game goes on. But its shared script context was locked with
`lock().unwrap()` at 214 non-test sites. One panic while holding the lock
poisons the mutex, and every later script call would then panic too.
`ClockBridge` had the same shape at three sites on the 1 kHz capture thread
and the per-frame session path. Found in Star Child's pre-production
hygiene pass, whose records were numbered 0038 to 0041; only this one and
0040 are cited in Flint.

## Decision

- Both crates gain a `lock_or_recover` helper:
  `PoisonError::into_inner`, plus one `tracing::error!` per process naming
  the ADR.
- The commit says the recovery is sound because of what the locks guard:
  - the script context holds always-valid snapshots written whole each
    frame, so the last-written state is safe to read after a writer
    panicked;
  - the clock bridge holds a self-repairing observation buffer: a bad fit
    refits within `REFIT_EVERY` ticks.
- Test modules keep `unwrap()` on purpose.

## Alternatives considered

- **Keep propagating the panic.** Rejected: one panic would become a panic
  on every later call, against the crate's policy.

## Consequences

- Healthy-path behaviour is unchanged by construction; the gate was the
  `flint-script` and `flint-music` suites, including the log-and-continue
  test.
- Since then: `api.rs` was split into `api/` modules, and the script helper
  is used across `flint-script` (`context`, `host`, `engine`, several `api`
  modules); the `lock().unwrap()` calls left are in test modules.

## Measured

- 214 non-test `lock().unwrap()` calls on the script context, 181 of them
  in `api.rs`'s registrars; three in `ClockBridge` (commit).

## Sources

- Commit
  [`993118d`](https://github.com/chrischaps/Flint/commit/993118d5796b57a78180b9faa585b8c9c0b15066)
  "Hygiene WS3.2: recover from mutex poisoning on script/bridge state
  (ADR 0039)"
- Vault: "2026-08-19 Starchild hygiene pass — determinism as a refactoring
  tool"
- Code now: `crates/flint-script/src/lib.rs` (`lock_or_recover`),
  `crates/flint-music/src/clock_bridge.rs` (`lock_or_recover`)
