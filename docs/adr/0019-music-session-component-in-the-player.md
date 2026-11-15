# ADR 0019: A scene declares its music session with a `music_session` component

Status: accepted, 2026-08-16 (commit
[`d95dc8b`](https://github.com/chrischaps/Flint/commit/d95dc8bab84aa5d76aeac229c91b653f061b0ef8)).
Backfilled 2026-11-12 (#374) from commits `d95dc8b` and `72e2b7e`, the code
that cites it, the book and the Vault note "Starchild F3 - Music Session in
the Player - 2026-08-16". The original record was in the Starchild repo, which
no longer exists; this is a reconstruction.

## Context

F2 proved the shared manager and the gamepad handoff with a throwaway spike
(`music_spike.rs`, a `--music-spike` flag, a `spike_f2` scene). F3 had to give
the real `flint-player` a session lifecycle and delete the spike. The Vault
note says ADR 0019 "records the decisions" of F3.

## Decision

- **A dynamic component.** A scene declares its suite with `music_session`
  (`manifest`, `chart`, and optional fields such as `lean_mode`,
  `ladder_config` and `bars`). It was the first game-side dynamic component:
  its schema is `schemas/components/music_session.toml` in the game, and the
  player reads it by name and checks the fields itself.
- **Lifecycle.** At scene load the player starts the reactive loop on the
  shared manager ([ADR 0017](0017-one-shared-audio-manager.md)), hands the
  pad to capture and down-samples its drain into `InputState`
  ([ADR 0018](0018-gamepad-handoff-and-inputstate-downsample.md)), and merges
  the ladder and reintegration visuals into the post overrides, script values
  winning. It tears down on natural finish or at a scene transition.
- **Headless degrades.** No audio device means no session, with a console
  notice; the rest of the scene runs.
- In `flint-music`: `ChartSession::open_shared` over a borrowed manager
  (`open()` unchanged), `stop_audio` (mandatory on a shared manager),
  `tick_with` with a per-event tap, `max_stem_skew`, and a field reorder so a
  drop without `finish()` no longer tore the audio down before the capture
  guard.
- **Deferred:** the component's `bindings` field.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The F2 spike was deleted in the same commit.
- Since then: [ADR 0020](0020-conducted-parameters-script-surface.md) closed
  the `bindings` deferral as validated, not loaded: the script it names loads
  through an ordinary `script` component. `tuning_config` is still parsed and
  not read. `quit_on_finish` came later ([ADR 0036](0036-quit-on-finish.md)).
- Since then: the schema stayed in the game. Flint's own `schemas/` has no
  `music_session.toml`, and the Starchild repo that held it no longer exists;
  the book's field table is the reference.
- The session is not yet one of the player's subsystems ([ADR 0117](0117-player-subsystems-and-one-game-step.md)); the
  player's `CLAUDE.md` names #745 for that.

## Measured

Live controller pass (`judgment-1786903119.jsonl`): 815 events, 0
monotonicity violations, 0.000 ms confirmed skew, pulse errors centred on
zero.

## Sources

- Commit [`d95dc8b`](https://github.com/chrischaps/Flint/commit/d95dc8bab84aa5d76aeac229c91b653f061b0ef8)
  "F3: session-in-player lifecycle — music_session component (ADR 0019);
  spike deleted"
- Commit [`72e2b7e`](https://github.com/chrischaps/Flint/commit/72e2b7ebf3b24b319a23df39744dbd0c893cd7bb)
  "F4: conducted-parameters script surface ([ADR 0020](0020-conducted-parameters-script-surface.md))"
- Vault: "Starchild F3 - Music Session in the Player - 2026-08-16"
- Code now: `crates/flint-player/src/player_app/music_session.rs`
  (`MUSIC_SESSION`, `MusicSession`),
  `crates/flint-player/src/player_app/script_commands.rs`,
  `crates/flint-player/src/player_app/state.rs` (`MusicState`),
  [Music Sessions](../book/src/concepts/music-sessions.md), "The
  `music_session` Component"
