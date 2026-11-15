# ADR 0083: Depth-tested world lines

Status: accepted, 2026-10-17 ([#51](https://github.com/chrischaps/Flint/pull/51)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #51's description and
commit `de85652`.

## Context

Games mark the world with lines: range rings, paths, selection outlines.
`draw_line_3d` projects them onto the HUD, so they're never hidden. The far
half of a ring round a statue crosses over the statue's front. The skeleton
and gizmo line pipelines are one-pixel `LineList`s, not usable as game
visuals.

## Decision

- API. `draw_world_line(x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)` in
  Rhai. The arguments and units are `draw_line_3d`'s: world points,
  display-space RGBA, width in logical points. Rust hosts call
  `SceneRenderer::set_world_lines(Vec<flint_core::world_line::WorldLine>,
  pixels_per_point)`. The player drains the scripts' lines each frame
  (then in `drive_script_ui`) and sets them.
- Where it draws. After the composite and FXAA, straight onto the final
  target, before the HUD. It tests against the frame's resolved
  single-sample depth (`LessEqual`, no depth write), so nearer geometry
  hides the lines and the lines never hide each other. Fog, bloom and
  tonemapping don't touch them. A renderer with MSAA but no HDR chain has
  no resolved depth, so it skips the pass.
- Geometry. On the CPU (`world_line_pipeline::build_vertices`). Each
  segment is projected with the frame's own camera, cut at the near plane
  (clip `z = 0`), and expanded into a screen-space quad with a 1 px
  feather, which the fragment shader fades to anti-alias the edge. A line
  under a pixel wide fades rather than thins.
- Depth slack. Each end is pulled toward the eye by 0.2% of its distance
  (`DEPTH_SLACK`), so a line lying on a surface doesn't z-fight it. That's
  20 cm at 100 m.
- Colour. Display-space, like the HUD's, linearised when the target is
  sRGB, so a line moved from `draw_line_3d` keeps its colour.
- Cost. The pipeline is built on the first frame that has lines, and the
  vertex buffer grows to the busiest frame (six vertices a segment).

## Alternatives considered

- Runtime meshes per ring
  ([ADR 0074](0074-runtime-meshes-and-frustum-culling.md)): no engine
  change, but the styling moves out of scripts, and an annulus in world
  units changes width with distance.
- Drawing in the HDR scene pass: fog, bloom and tonemapping would shift the
  colours, and the pass would need MSAA plumbing.

## Consequences

- Opt-in: nothing draws unless a script or host adds lines.
- A line meant to lie on a raised surface must still be placed at or above
  it. There's no layer and no clip rect, unlike the HUD functions.
- The record said one extra pass and submit, only on frames with lines.
  Since then: #139 made the frame one submission, and `encode_to` records
  the world lines into the frame's encoder.

## Sources

- [#51](https://github.com/chrischaps/Flint/pull/51) / commit
  [`de85652`](https://github.com/chrischaps/Flint/commit/de85652bce6fc0acbe8d318b5b656b035ebbaef8)
  "Depth-tested world lines: draw_world_line (ADR 0083)"
- Code now: `crates/flint-core/src/world_line.rs`,
  `crates/flint-render/src/world_line_pipeline.rs`,
  `world_line_shader.wgsl`, `crates/flint-render/src/scene_renderer/mod.rs`
  (`render_world_lines`); book:
  [Scripting](../book/src/concepts/scripting.md) (Lines in the world),
  [Rendering](../book/src/concepts/rendering.md) (World lines)
