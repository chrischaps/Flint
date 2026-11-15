# ADR 0062: `player_app` is split into named lifecycle files

Status: accepted, 2026-08-20 (commits
[`7742022`](https://github.com/chrischaps/Flint/commit/7742022b0de73a3688479f886bed51abc2da2476)
to [`e5184cd`](https://github.com/chrischaps/Flint/commit/e5184cdc7416ba8a68e636b72d67d9c0fe91a739)).
Backfilled 2026-11-12 (#374) from the book line that cites it and the ten
`refactor(player_app)` commits of 2026-08-20, none of which names it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Most of `flint-player`'s `PlayerApp` lived in `player_app/mod.rs`, beside a
few siblings such as `music_session.rs` and `scene_loading.rs`. The first
commit calls the split "tech-debt entry 1". The book's architecture page
says the module "is decomposed into named lifecycle files (ADR 0062)"; the
commits that did it landed the same day, right after ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md).

## Decision

Two stages, each commit with no behaviour change:

- **Stage 1, pure motion.** Bodies move verbatim into siblings:
  `debug_panels`, `events` (the winit `ApplicationHandler`, gamepad
  polling, rebind capture, cursor capture), `transition`,
  `script_commands`, `init` (construction and loading) and `frame` (render,
  tick, HUD). `pub(super)` is added only where a sibling calls across.
  `mod.rs` came down to 215 lines: module declarations, shared constants and
  the `PlayerApp` struct, whose field order is untouched because of
  [ADR 0017](0017-one-shared-audio-manager.md)'s kira drop order.
- **Stage 2, named steps.** `tick` becomes eleven named sub-steps,
  `window_event` per-arm handlers, `execute_scene_transition` three phases
  (`teardown_current_scene`, `load_target_scene`, `bring_up_scene`) and
  `initialize` five steps. Ordering contracts from earlier ADRs (0017,
  0018, 0020) read at the call site.
- Blocks were extracted verbatim and checked as a multiset; the function
  census matched the baseline.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Since then: [ADR 0117](0117-player-subsystems-and-one-game-step.md)
  (#724) went further. `PlayerApp`'s fields are grouped, optional features are `Subsystem`s, and a frame is
  `game_core::step` over a `GameCore` with no window. Startup and every
  transition bring a scene up through `enter_scene` in `scene_entry.rs`
  (#723), which replaced `load_target_scene` and `bring_up_scene`;
  `teardown_current_scene` remains in `transition.rs`. The book's list of
  files predates these.

## Sources

- Commits
  [`7742022`](https://github.com/chrischaps/Flint/commit/7742022b0de73a3688479f886bed51abc2da2476)
  "refactor(player_app): move debug-panel creation to debug_panels.rs (pure
  motion)",
  [`d443063`](https://github.com/chrischaps/Flint/commit/d4430633645610d5e7c8733c2c35c63eee43dae5)
  "refactor(player_app): move frame loop to frame.rs (pure motion) — Stage
  1 complete",
  [`e5184cd`](https://github.com/chrischaps/Flint/commit/e5184cdc7416ba8a68e636b72d67d9c0fe91a739)
  "refactor(player_app): decompose initialize into named steps — Stage 2
  complete", and the seven between them
- Code now: `crates/flint-player/src/player_app/` (`mod.rs`, `init.rs`,
  `frame.rs`, `events.rs`, `transition.rs`, `scene_entry.rs`,
  `game_core.rs`)
- Book: [Architecture overview](../book/src/architecture/overview.md),
  "flint-player"
