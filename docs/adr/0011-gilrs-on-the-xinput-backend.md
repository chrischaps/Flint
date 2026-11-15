# ADR 0011: gilrs runs on XInput, and a silent input path says so

Status: accepted, 2026-08-15 (commit
[`d1314a7`](https://github.com/chrischaps/Flint/commit/d1314a72d96dac69c63a2dfc2b00adfdff7674e1)).
Backfilled 2026-11-12 (#374) from commits `d1314a7`, `2fc8606` and `9ab343e`
(#113), the comment on `gilrs` in the workspace `Cargo.toml`, and the Vault
note "2026-08-16 Audio Gradient - Tuning by Wobble Not Offset". The original
record was in the Starchild repo, which no longer exists; this is a
reconstruction.

## Context

The capture thread ([ADR 0008](0008-gamepad-capture-on-its-own-thread.md))
and `play-chart --spike-input-secs` recorded zero events. gilrs's default
Windows backend, Windows.Gaming.Input, delivers input only to the process
that owns the foreground window, and these were console programs.

## Decision

- The workspace `gilrs` dependency turns default features off and enables
  only `xinput`. XInput polls regardless of focus.
- The spike and the capture thread print and log the gamepads they find, so a
  backend or device mismatch shows up at once.

The failure class, a backend alive with zero devices and no error, became a
standing rule in the code: make it loud. After a feel session that recorded
zero events in 126 s, commit `2fc8606` added a connected-gamepad check before
the session, a one-time NO INPUT warning if nothing arrives by bar 2, a NO
INPUT marker on bar lines, and a red breathing frame edge in the harness
window.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Code cites the rule by number: `flint-input-capture` reports zero devices
  loudly, the rumble path keeps lifetime evidence counters, and
  `ChartSession` surfaces NO INPUT. The audio gradient later warned at
  startup when its target bus is silent, "the ADR 0011 pattern" (Vault).
- Since then (#100, PR #113): on other targets the backend is chosen by
  target (evdev, IOKit, the browser Gamepad API on wasm32). `xinput` is a
  no-op there but stays on, because gilrs-core 0.6.7's "needs xinput or wgi"
  `compile_error` is not gated to Windows.

## Measured

The session that prompted `2fc8606`: zero input events in 126 s.

## Sources

- Commit [`d1314a7`](https://github.com/chrischaps/Flint/commit/d1314a72d96dac69c63a2dfc2b00adfdff7674e1)
  "Fix silent zero-event input capture: switch gilrs to XInput backend (ADR
  0011)"
- Commit [`2fc8606`](https://github.com/chrischaps/Flint/commit/2fc8606a013ef2b9c2ddb33ea41696ca075155bc)
  "Make silent input death loud; arm the ladder from measured real play"
- [#113](https://github.com/chrischaps/Flint/pull/113) / commit
  [`9ab343e`](https://github.com/chrischaps/Flint/commit/9ab343e4630d41e4eade66de65ca0d1fe9a17aa4)
  "Web platform shims: time, gamepads, settings (#100)"
- Vault: "2026-08-16 Audio Gradient - Tuning by Wobble Not Offset"
- Code now: `Cargo.toml` (`gilrs`), `crates/flint-input-capture/src/lib.rs`,
  `crates/flint-input-capture/src/rumble.rs`,
  `crates/flint-music/src/chart_session.rs`, `docs/design/web.md`
