# ADR 0017: Music and world sound share one Kira AudioManager, dropped in order

Status: accepted, 2026-08-16 (commit
[`187e756`](https://github.com/chrischaps/Flint/commit/187e75638922f99495c2912a42998f7cd6a9cb05)).
Backfilled 2026-11-12 (#374) from commits `187e756` and `d95dc8b`, the code
that cites it, and the Vault notes "Starchild F2 Player Integration Spike -
2026-08-16" and "Starchild F3 - Music Session in the Player - 2026-08-16". The
original record was in the Starchild repo, which no longer exists; this is a
reconstruction.

## Context

Until Phase 4 every music host opened its own Kira manager
([ADR 0007](0007-suite-playback-keeps-its-own-manager.md)). In the player, a
second manager would mean a second device stream beside `flint-audio`'s
spatial sound. F2 was the spike for this risk and for the gamepad handoff
([ADR 0018](0018-gamepad-handoff-and-inputstate-downsample.md)).

## Decision

- **One manager.** `AudioEngine::manager_mut()` lends the player's Kira
  manager to the music suite, and the session opens on it
  (`ChartSession::open_shared`). Stems and `audio_source` sounds share one
  device and one clock, and one master low-pass muffles both.
- **Drop order is producer → handles → device.** Handles created on the
  manager must drop before the engine that owns it. This is held by field
  declaration order at each layer: `ChartSession`'s input guard before its
  suite's handles; `PlayerApp`'s music state before `audio`.
- **Teardown stops the music first.** A scene transition stops the session
  before `audio.clear()`.
- **Skew readings are double-read.** Polling `StaticSoundHandle::position()`
  across buses at frame rate sometimes shows a one-frame disagreement of
  exactly 10.000 ms: one audio-callback buffer (480 samples) of Kira's
  position reporting being stale, not de-sync. An immediate re-read collapses
  it to 0.000 ms, so skew is believed only after a re-read.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- F3 found that on a shared manager, dropping handles does not stop sounds;
  the CLI had only been saved by its own manager dying with the session.
  `ChartSession::stop_audio(fade_ms)` is mandatory on every player teardown
  path (Vault, F3).
- Every player session prints "max confirmed skew", the F2 pass criterion
  made permanent.
- Since then: the order rule constrains later refactors. The 2026-08-20
  `player_app` split kept field order and split `execute_scene_transition`
  with the music stop before `audio.clear`; [ADR 0117](0117-player-subsystems-and-one-game-step.md)'s `GameCore` and its
  subsystems are declared before `audio` for this rule.
- Since then (#136): on the web, `manager_mut()` returns `None` when the mix
  runs in the browser's AudioWorklet, so a scene with a `music_session` keeps
  the main-thread cpal stream.

## Measured

F2: 0.000 ms stem skew across full runs with a spatial one-shot every 2 s
(23/23 ok), and no device contention.

## Sources

- Commit [`187e756`](https://github.com/chrischaps/Flint/commit/187e75638922f99495c2912a42998f7cd6a9cb05)
  "F2 PASSED: player integration spike — shared AudioManager + gilrs handoff
  (ADRs 0017, 0018)"
- Commit [`d95dc8b`](https://github.com/chrischaps/Flint/commit/d95dc8bab84aa5d76aeac229c91b653f061b0ef8)
  "F3: session-in-player lifecycle — music_session component ([ADR 0019](0019-music-session-component-in-the-player.md));
  spike deleted"
- Commits [`53fb22c`](https://github.com/chrischaps/Flint/commit/53fb22cd15d382dc2808d458c0f1005264fa41a4),
  [`d443063`](https://github.com/chrischaps/Flint/commit/d4430633645610d5e7c8733c2c35c63eee43dae5),
  [`609973f`](https://github.com/chrischaps/Flint/commit/609973f1ec4d9515350d4e3148d984dce81f9a53)
  (2026-08-20 `player_app` refactors)
- Vault: "Starchild F2 Player Integration Spike - 2026-08-16"; "Starchild F3 -
  Music Session in the Player - 2026-08-16"
- Code now: `crates/flint-audio/src/engine.rs` (`AudioEngine::manager_mut`),
  `crates/flint-music/src/chart_session.rs` (`ChartSession::open_shared`,
  `stop_audio`, `max_stem_skew`),
  `crates/flint-player/src/player_app/state.rs` (`MusicState`),
  `crates/flint-player/src/player_app/transition.rs`,
  [ADR 0117](0117-player-subsystems-and-one-game-step.md),
  [Audio](../book/src/concepts/audio.md),
  [Music Sessions](../book/src/concepts/music-sessions.md), "Lifecycle"
