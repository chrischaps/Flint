# ADR 0037: A Manifest Map strip shows the whole suite and this run's history

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commits `86d21ec` and `a08af57`, Vault note
"2026-08-17 Manifest Map timeline panel", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The Music Guide ([ADR 0035](0035-music-guide-debug-panel.md)) shows the next few beats. Star Child's loop is
fail, rewind, re-enter, and nothing showed where in the suite a run was or
where it had rewound.

## Decision

- A "Manifest Map" debug panel in `flint-player`, toggled with Backslash: a
  full-width bottom strip of the whole suite, with sections, a bar ruler,
  tempo and meter changes, re-entry points, a playhead, and this run's
  judged pulses (hit and miss ticks) and seams with their rewind
  connectors. Behind `debug-hud`, like the guide.
- The data follows the guide's pattern: `TimelineMap`, built once at
  session open from `SuiteManifest`, plus a cheap per-frame `TimelineFrame`.
  Bar lines are precomputed, so the panel does no tempo math and stays
  right across tempo and meter changes.
- Histories that must be complete (seams) accumulate whether or not the
  panel is open. That is safe because nothing on a deterministic path reads
  them: they are write-only `Vec`s on `ChartCore`, and the golden suites
  stayed byte-identical.
- `flint_debug_ui::DebugPanel` gains a defaulted `layout()` (`SideRight`,
  `Bottom`), so a wide panel can claim the bottom strip without touching
  the others.

## Alternatives considered

- **Feeding history only while the panel is open.** Rejected for seams,
  which have to be complete (Vault).

## Consequences

- The Vault note records the player's key inventory then: Backquote for the
  guide, Backslash for the map, F1-F12 all taken.
- Since then: commit
  [`a08af57`](https://github.com/chrischaps/Flint/commit/a08af578df9b4785cbbb83713040ad75ecaa006d)
  moved the timeline types (`TimelineMap`, `TimelineSection`, `BarMark`,
  `TempoMark`, `SeamMarkKind`, `SeamMark`, `PulseMark`, `TimelineFrame`)
  out of `chart_session.rs` into `flint-music`'s `timeline` module, with
  re-exports so callers didn't change.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Commit
  [`a08af57`](https://github.com/chrischaps/Flint/commit/a08af578df9b4785cbbb83713040ad75ecaa006d)
  "Hygiene WS2a.4: lift the debug-timeline cluster out of chart_session"
- Vault: "2026-08-17 Manifest Map timeline panel"
- Code now: `crates/flint-music/src/timeline.rs` (`TimelineMap`,
  `TimelineFrame`), `crates/flint-player/src/player_app/timeline_panel.rs`,
  `crates/flint-debug-ui/src/lib.rs` (`DebugPanel::layout`),
  [Music Sessions](../book/src/concepts/music-sessions.md), "Debug Surfaces"
