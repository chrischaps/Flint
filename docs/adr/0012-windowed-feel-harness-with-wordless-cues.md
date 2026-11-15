# ADR 0012: The feel harness gets a window with wordless visual cues

Status: accepted, 2026-08-15 (commit
[`e652fdc`](https://github.com/chrischaps/Flint/commit/e652fdcdd025b44679335988debaa4b6526f2eeb)).
Backfilled 2026-11-12 (#374) from commits `e652fdc` and `2e782c4` and the
module doc of `play_chart_window.rs`. The original record was in the
Starchild repo, which no longer exists; this is a reconstruction.

## Context

`flint play-chart` ran in a console. Gamepad-to-keyboard mappers (Steam's
desktop layout, Legion Space) turn pad presses into OS keystrokes, which were
typing into the terminal mid-session; an A press became Enter and fired a
config reload on every pulse (`2e782c4` hardened stdin against it). A
stem-only build also gave the player nothing to see, and the game's design
forbids meters, icons and grades.

## Decision

- `play-chart --window` opens a bare wgpu window (one fullscreen-triangle
  WGSL shader) over the same session the console loop drives. The console
  loop was refactored into a front-end-agnostic `ChartSession` for it.
- Everything drawn is field and glow, nothing a readout: a warm glow where
  the chart wants the lean, a cooler, smaller glow for the player's actual
  lean (togetherness reads as the two merging), beat and bar breathing from
  the `Conductor`, a ring blooming on each pulse, and saturation that drains
  as coherence falls.
- A focused window absorbs the mappers' keystrokes harmlessly. Debug numbers
  stay on stdout, as in console mode.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The window became the debug surface for later work: the ladder's visual
  rungs (`e18d03a`) and the NO INPUT frame edge
  ([ADR 0011](0011-gilrs-on-the-xinput-backend.md)) draw in it.
- The front-end-agnostic `ChartSession` was later hoisted into `flint-music`
  so the player could host it
  ([ADR 0016](0016-reactive-loop-in-flint-music.md)).
- Its ash-grey desaturation formula was matched by the engine's composite
  shader ([ADR 0021](0021-ladder-visuals-through-the-post-stack.md)).

## Sources

- Commit [`e652fdc`](https://github.com/chrischaps/Flint/commit/e652fdcdd025b44679335988debaa4b6526f2eeb)
  "Windowed feel harness: play-chart --window with wordless visual cues (ADR
  0012)"
- Commit [`2e782c4`](https://github.com/chrischaps/Flint/commit/2e782c4b198ea6c9bf685ff33833b6910b2e375c)
  "Harden play-chart stdin against gamepad-to-keyboard mappers"
- Code now: `crates/flint-cli/src/commands/play_chart_window.rs`
  (`run_windowed`), `crates/flint-cli/src/commands/play_chart.rs`,
  [Music CLI](../book/src/cli-reference/music.md)
