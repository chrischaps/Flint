# ADR 0046: The scene viewer applies the scene's `[post_process]` block on load

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0` and its diff, Vault note
"2026-08-19 Clay Look Orbit Variant", and the book.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.
Amended by [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) (the toggle moved into the F4 menu).

## Context

`flint edit` drew a scene with the viewer's own post-processing defaults, so
a scene authored around DoF, vignette or fog looked different there than in
the player. The clay-look spike ([ADR 0042](0042-clay-look-research-spike.md)) was judged through those effects.

## Decision

From the commit's diff:

- The viewer applies the scene's authored `[post_process]` block on load,
  and re-derives it when the scene hot-reloads, so edits to the block show
  like any other scene edit.
- F11 toggled between the authored block and the viewer's default config,
  which the viewer keeps for that.
- The copy from `PostProcessDef` to `PostProcessConfig` moved from the
  player into `flint-render` as `post_process_config_from_def`, so the
  player, `flint render` and the viewer apply the block through one
  function. The player re-exports it under its old path.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The viewer applied the authored post but still ignored the authored
  `[camera]`, so DoF-heavy scenes opened as a blur at the default distance.
  [ADR 0051](0051-scene-viewer-opens-on-authored-camera.md) closed that gap; the Vault note says "post and camera are one
  composition".
- Since then: [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) replaced the viewer's F-key toggles with the F4
  Rendering & Effects menu, where the switch is "Authored post".

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs [0042](0042-clay-look-research-spike.md)-[0047](0047-viewer-dof-follow.md))"
- Vault: "2026-08-19 Clay Look Orbit Variant"
- Code now: `crates/flint-render/src/postprocess.rs`
  (`post_process_config_from_def`), `crates/flint-viewer/src/app.rs`,
  `crates/flint-player/src/player_app/scene_loading.rs`,
  [Post-Processing](../book/src/concepts/post-processing.md),
  [Debug Panels](../book/src/guides/debug-panels.md)
