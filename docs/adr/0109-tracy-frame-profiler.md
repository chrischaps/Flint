# ADR 0109: A frame profiler behind flint-player's `profile` feature, through Tracy

Status: accepted, 2026-10-21 ([#83](https://github.com/chrischaps/Flint/pull/83)).
Backfilled 2026-11-12 (#374) from PR #83, whose body holds the ADR text for
0107 to 0109, and commit `35afb2a`.

## Context

There was no way to see the frame loop's CPU time scope by scope.

## Decision

A `profile` feature on flint-player: the `profiling` crate with
`profile-with-tracy`.

- `#[profiling::function]` on the frame loop's phases (`tick`,
  `run_extension_pre_updates`, `tick_music_session`, `tick_fixed_physics`,
  `run_scripts`, `drive_script_ui`, `tick_av_systems`, `tick_procgen`,
  `sync_renderer`, `render`, `render_hud`) and the renderer's
  (`update_from_world`, `render_to`, `render_shadow_pass`,
  `render_main_pass`, `render_postprocess`).
- `profiling::finish_frame!()` after each `RedrawRequested`.
- The client is started in `PlayerApp::new`.
- `tracy-client` is named directly only to add `only-localhost` (it listens
  on 127.0.0.1:8086, so no firewall prompt) and `ondemand` (nothing is kept
  until a viewer connects).
- `profiling` was already in the tree through wgpu, so wgpu's own scopes come
  with it.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Without the feature the attributes compile to nothing. `profiling` is the
  only new dependency of flint-render and flint-player, and it was already in
  the lockfile.
- Games depend on `profiling` to scope their own code, and it reaches Tracy
  under the same feature (`profile = ["flint-player/profile"]` in a game's
  binary).
- Under the feature on Windows, Tracy prints `SymInitialize FAILED with code
  87` once at start in some processes (test harnesses). Profiling still
  works.

## Sources

- [#83](https://github.com/chrischaps/Flint/pull/83) / commit
  [`35afb2a`](https://github.com/chrischaps/Flint/commit/35afb2a254c411881cf581bc4e2f5be134cbbef6)
  "[#374] Flint: game debug panels, GPU timing, a frame profiler (ADRs 0107-0109)"
- Code now: `crates/flint-player/Cargo.toml` (`profile`), `Cargo.toml`
  (`profiling`), `crates/flint-player/src/player_app/init.rs`,
  `crates/flint-player/src/player_app/events.rs`
- Book: [Profiling](../book/src/guides/profiling.md)
