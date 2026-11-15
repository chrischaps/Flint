# ADR 0104: Depth-tested world text, drawn by the player with egui's fonts

Status: accepted, 2026-10-21 ([#78](https://github.com/chrischaps/Flint/pull/78)).
Backfilled 2026-11-12 (#374) from PR #78, whose body holds the ADR text, and
commit `d2788e6`.

## Context

Script text is HUD. `draw_text_opts` at a `world_to_screen` point is drawn
over everything, so a name written on something in the world (a shop's sign,
a name tag) shows through whatever stands in front of it. [ADR 0083](0083-depth-tested-world-lines.md) gave
scripts depth-tested lines, but not text. The Capitol worked round it with a
hand-written occlusion model of its market stalls: a ray walked through
boxes, and the name was drawn whole or not at all (TheCapitol#274, H30).

## Decision

`draw_world_text(x, y, z, text, size, opts)`:

- `opts` takes `draw_text_opts`'s keys (`font`, `color`, `align`, `stroke`,
  `spacing`, `shadow`; no `layer`), plus `valign`. Centred on the point by
  default.
- **Billboard** (the default): faces the screen at `size` logical points and
  takes the point's depth, like a name tag.
- **On a plane**: with `facing` (and `up`) it lies in that plane, read from
  the front only. Every length is in world units. The em is rasterised at the
  size it spans on screen, in whole pixels from 4 to 128.
- Data: `flint_core::world_text::{WorldText, TextPlane}`. Scripts queue them
  and `ScriptSystem::drain_world_texts()` hands them to the host.

It's the player's, not the renderer's, because it's egui's text: the same
fonts, colours, strokes and shadows as HUD text, with no second font stack.

1. Inside the HUD's egui pass each label is laid out with egui's fonts, so
   its glyphs go up with that pass's font-atlas delta.
2. After the pass each label is tessellated on its own and its vertices moved
   into clip space. A billboard keeps its screen layout at the anchor's
   depth, snapped to a whole pixel; a plane's label is projected with its
   true `w`. A label reaching behind the camera is dropped whole.
3. They're drawn onto the HUD's target in one pass before the egui pass,
   against `RenderContext::depth_view` (`LessEqual`, no depth write), with
   egui-wgpu's blend and colour maths and egui's font-atlas bind group.
4. As with world lines, the anchor is pulled toward the eye by `DEPTH_SLACK`
   (0.2%), so text painted on a surface doesn't fight it.

`SceneRenderer::frame_depth_ready()` says whether the depth view holds the
frame's single-sample depth. It doesn't with MSAA and no HDR chain, and then
world text isn't drawn.

## Alternatives considered

- **Draw it in the renderer.** The record gives the reason against: it would
  need a second font stack beside egui's.
- **A game-side occlusion model**, the workaround in use (The Capitol's
  boxes): text drawn whole or not at all, and a second description of the
  world to keep in step.

## Consequences

- Opt-in; no new dependencies.
- Fog, bloom and tonemapping don't touch it, as with world lines. Like HUD
  text, it's unlit.
- Text on a plane at a changing on-screen size asks egui for glyphs at a few
  whole-pixel sizes; egui's atlas holds them and clears itself if it fills.
- There's no minimum size: scripts decide when text is too small to read.
- Since then: a wide stroke or a glow also samples the HUD's halo picture
  (#626).

## Sources

- [#78](https://github.com/chrischaps/Flint/pull/78) / commit
  [`d2788e6`](https://github.com/chrischaps/Flint/commit/d2788e6b69e9abf3ff7a0792b67695b912b8918c)
  "[H30] Depth-tested world text: draw_world_text (ADR 0104)"
- Code now: `crates/flint-core/src/world_text.rs`,
  `crates/flint-player/src/player_app/world_text.rs`,
  `crates/flint-player/src/player_app/world_text_shader.wgsl`,
  `crates/flint-render/src/scene_renderer/particles.rs` (`frame_depth_ready`)
- Book: [Rendering](../book/src/concepts/rendering.md), "World text";
  [Scripting](../book/src/concepts/scripting.md), "Text in the world"
