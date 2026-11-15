# ADR 0016: The reactive loop moves out of the CLI into `flint-music`

Status: accepted, 2026-08-16 (commit
[`963b2b0`](https://github.com/chrischaps/Flint/commit/963b2b000fc97070421941485268bff027faf77a)).
Backfilled 2026-11-12 (#374) from commit `963b2b0`, the module docs it left,
and the Vault note "2026-08-16 Star Child Phase 4 planned — the prototype
chooses the harder road". The original record was in the Starchild repo,
which no longer exists; this is a reconstruction.

## Context

The reactive loop, chart evaluation → judgment → coherence → ladder →
reintegration, lived in the `flint-cli` binary as `ChartSession`, and
`replay-chart --render` kept a hand copy of it. Phase 4 planning chose full
`flint-player` integration over growing the windowed harness into the scene,
so the prototype would stress the real risks (Vault). The player had to be
able to host a session.

## Decision

- `ChartSession` and `ChartCore` move to `flint-music/src/chart_session.rs`.
- `ChartCore` is the loop body with no player and no device.
  `replay-chart --render` now runs it, not a copy.
- `ChartSession` takes input as a plain `Receiver<InputEvent>`, so no gilrs
  in the library. The capture handle is parked as an opaque guard and
  dropped before audio teardown.
- The latency and calibration helpers move with it; `latency_files.rs` is
  deleted.
- The CLI commands are thin wrappers. Notices are returned and printed by the
  front ends.

## Alternatives considered

Not recorded in the surviving text for the hoist itself. The Phase 4 plan's
fork, recorded in the Vault: growing the windowed feel harness into the scene
was cheaper and avoided the player app's structural conflicts; Chris chose
player integration because the player would be needed eventually and the
work would transfer.

## Consequences

- One loop for the console, the window, offline replay and, from F3, the
  player ([ADR 0019](0019-music-session-component-in-the-player.md)).
- Since then: `ChartCore::conducted_frame` produces the scripts' snapshot on
  both the live and the offline path
  ([ADR 0020](0020-conducted-parameters-script-surface.md)).

## Measured

Byte-identical before and after: the reactive-render WAV, judgment JSONL and
stdout on `synthetic-fail-demo`, and the synthetic-perfect JSONL and stdout.

## Sources

- Commit [`963b2b0`](https://github.com/chrischaps/Flint/commit/963b2b000fc97070421941485268bff027faf77a)
  "F1: hoist ChartSession/ChartCore into flint-music (ADR 0016)"
- Vault: "2026-08-16 Star Child Phase 4 planned — the prototype chooses the
  harder road"
- Code now: `crates/flint-music/src/chart_session.rs` (`ChartSession`,
  `ChartCore`), `crates/flint-cli/src/commands/play_chart.rs`,
  `crates/flint-cli/src/commands/replay_chart.rs`
