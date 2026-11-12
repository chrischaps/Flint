# ADR 0066: A scene can opt out of the blanket audio preload

Status: accepted, 2026-09-03 (commit
[`81e54fc`](https://github.com/chrischaps/Flint/commit/81e54fc196f95adeac7c64a0b5a6b69e503e0440)),
the date it reached Flint.
Backfilled 2026-11-12 (#374) from commit `81e54fc` and the book lines that
cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction. The decision was made in Starchild's engine subtree.

## Context

At scene load the player preloaded every file under `audio/`, so that a
script could play any of them by name. A scene with a large audio folder
paid for all of it before it started.

## Decision

- `[scene] preload_audio`, default `true`. `false` skips the blanket
  preload.
- Sounds named by `audio_source` components and music-session stems still
  load through their own paths; only the convenience preload for
  script-triggered sounds is skipped.
- The default keeps the earlier behaviour. The key is carried by
  `flint-scene`'s format, prefab and saver, and read by the player at start
  and at each transition and by `flint play`.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A scene that opts out starts without the wait, and a script that plays a
  file nothing else names won't find it.
- Since then: the preload lists `audio/` through the project's asset
  search ([ADR 0114](0114-project-manifest-and-one-resolver.md)): every
  `audio/` the scene can see, each file kept as `audio/<file name>`.

## Sources

- Commit
  [`81e54fc`](https://github.com/chrischaps/Flint/commit/81e54fc196f95adeac7c64a0b5a6b69e503e0440)
  "music/player/scene: upstream Starchild's five engine features"
- Code now: `crates/flint-scene/src/format.rs` (`preload_audio`),
  `crates/flint-player/src/player_app/scene_loading.rs`,
  `crates/flint-player/src/player_app/state.rs` (`SceneSettings`)
- Book: [Scenes](../book/src/concepts/scenes.md);
  [Audio](../book/src/concepts/audio.md), "Preloading"
