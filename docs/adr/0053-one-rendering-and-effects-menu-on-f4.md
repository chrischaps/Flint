# ADR 0053: One Rendering & Effects menu on F4 replaces the per-effect keys

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27`, commit `ec3f956` and
the code comments that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Render and post-process debug controls were scattered over function keys:
F1 cycled the debug mode, F4 toggled shadows, F5-F10 toggled single
effects, F12 Kuwahara, F11 the authored post and F12 DoF-follow in the
viewer. A key can only flip something, so none of the non-binary
parameters (SSAO radius and samples, DoF focus and range, the grade, render
mode parameters, shadow resolution, lighting levers, FOV) could be tuned
live. [ADR 0035](0035-music-guide-debug-panel.md) had deferred folding the F-keys together.

## Decision

- One egui window, the Rendering & Effects panel, opened with F4 in both
  `flint-player` and `flint edit`'s scene viewer. The old F1 and F5-F12
  per-effect keys are removed.
- It holds every render and post toggle with its parameters: SSAO, DoF,
  bloom, fog, grade/grain/FXAA, Kuwahara, render modes, shadows and
  resolution, lighting levers, FOV, debug shading.
- Ownership model: while the panel is clean the host refreshes it from live
  renderer state every frame ("live mirror"); an edit is applied back
  ("write-through"), routed by per-group change flags to the right renderer
  call. A value a script drives is visibly taken back the next frame, unless
  the player-only "freeze script post overrides" switch is on, which skips
  the per-frame script stamp.
- The panel lives in `flint-debug-ui`, which gained a `flint-render`
  dependency, and is gated by the `debug-hud` feature.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The panel shows who owns a value rather than hiding a script's override.
- The book's keymaps were corrected in `ec3f956`: six pages still had the
  old rows.
- Since then: the panel grew a Particles section in the viewer
  ([ADR 0068](0068-particle-effect-assets-and-deterministic-sim.md)),
  contour and shading boil, and a Styles section; Kuwahara moved there
  (F4 -> Styles). The Styles panel follows the same ownership model. F5-F8
  came back for Living Props. A debug key with nothing to act on now says so
  on screen (#367).

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs 0053-0061)"
- Commit
  [`ec3f956`](https://github.com/chrischaps/Flint/commit/ec3f95635a0f3a63010738aa4403a1f627d9b2cb)
  "docs: bring the book up to date with engine changes since 2026-07-28"
- Code now: `crates/flint-debug-ui/src/render_panel.rs`,
  `crates/flint-debug-ui/src/styles_panel.rs`,
  `crates/flint-player/src/player_app/frame.rs` (freeze switch),
  `crates/flint-viewer/src/app.rs`
- Book: [Debug Panels](../book/src/guides/debug-panels.md);
  [Post-Processing](../book/src/concepts/post-processing.md)
