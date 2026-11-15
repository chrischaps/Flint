# ADR 0120: Comic panels are views swapped into one renderer

Status: accepted, 2026-11-12 (#880)

## Context

A comic page tells one beat in several panels at once: a wide shot beside a
close-up, the live world in one frame and the drawn world in the next. Flint
drew one camera a frame. `SceneRenderer::encode_to` takes one `Camera`, and
the whole post chain, styles included, runs over the whole target. A HUD could
draw panel borders over one picture, but not a second shot or a second look.

The renderer's single-camera assumption is everywhere. #880's spike listed what
is keyed to the camera or the frame's size before any code:

| State | Keyed to | Where |
|---|---|---|
| HDR chain, bloom mips, SSAO and volumetric targets, FXAA, the style targets (ping-pong, mask, albedo, every style's `target()` and history), the post bind groups | size | `PostProcessResources` |
| MSAA colour and depth | size | `SceneRenderer::msaa_color` / `msaa_depth` |
| The ocean's grab copies | size | `ocean_grab_*` |
| Scaled depth and its upscale | size and render scale | `scaled_depth` / `depth_upscale` |
| The style mask's and albedo's 4x targets | size | inside `StyleMaskPass` / `StyleAlbedoPass` |
| The selection mask | size | `SelectionOutline` |
| Shadow cascades, grass blades, culling, LOD, transparent order, camera and light uniforms | camera | recomputed by each `encode_to` |
| Write-once uniforms (texel sizes) | the cache that wrote them | `BindCache::first_write`, into pipeline-owned buffers |
| Every uniform the frame writes through `queue.write_buffer` | the submission | lands before the next `submit` |

The last row is the hard one. #139 made a frame one submission, and the
renderer's own note says it: encoding two cameras into one submission shows
the second camera's uniforms to both. About 66 `write_buffer` sites across the
renderer and its styles write one value a frame.

## Decision

1. **The layout is a scene table.** `[panels]` holds the page (`gutter`,
   `gutter_color`, `border`, `border_color`, in points and display colours).
   Each `[panels.panel.<name>]` holds a panel: `rect` as shares of the page,
   `order`, a camera (`position`, `target` and `fov`, or the frame camera's
   where unset), `follow` (an entity whose world origin those are measured
   from), and its own `look` and `post_process` laid over the scene's. A panel
   with neither draws with the frame's post-processing as it stands (F4,
   scripts and all). `flint_render::panels::PanelLayout` parses and checks it.
   `schemas/scene/panels.toml` declares it.
2. **A panel is a view swapped into the one renderer.** Everything in the
   table above that is keyed to size is a `ViewSlot` (`scene_renderer/panels.rs`).
   Each panel holds its own slot, made once by the same `resize_postprocess`
   the window uses, plus a picture and a depth of the panel's size. Around the
   panel's draw the renderer swaps the slot in, swaps in the panel's own config
   (under the quality caps, on the frame's boil), and calls the unchanged
   `encode_to` with the panel's camera. So a style in a panel sees that panel's
   size, depth, mask and camera and keeps its history per panel. What is
   recomputed per camera (shadows, grass, culling) is recomputed per panel, and
   draws, meshes, textures and pipelines are shared. Views are kept by panel
   name for the scene, so cutting back to a layout makes nothing. A size,
   format, MSAA or render-scale change remakes only that view.
3. **One submission a panel.** Each panel but the last is encoded on its own
   encoder and submitted, so its uniforms land before the next panel writes
   its own. The last panel and the page go into the caller's encoder. N panels
   are N submissions. A scene without `[panels]` takes the old path, still one
   submission. The GPU timer's scopes are the last panel's.
4. **The page is one pass.** `PanelCompositor` clears the frame to the gutter,
   then draws a full-screen triangle a panel, scissored to the panel's outer
   rectangle. It copies the picture texel for texel (`textureLoad`) inside and
   draws the border's ink around it. Its uniforms are 16 dynamic-offset slots
   in one buffer, and each picture's bind group is made with its picture. The
   HUD draws over the page afterwards, as before.
5. **One entry for hosts.** `encode_frame_to` / `render_frame_to` take the
   world (for `follow`) and draw either way. The player, `flint render`'s
   stills, `--frames` and the Stage path call it. Hosts set the layout at scene
   load (`set_panels(layout, base_def)`, which forgets the last scene's views).
   Scripts change it mid-scene through `set_panel_layout`.
6. **Scripts cut.** `set_panels(map)`, `set_panel(name, map)`,
   `remove_panel(name)` and `clear_panels()` (the `panels` module) queue
   commands that `CoreCommands` applies to the lent renderer, the same for the
   player and `flint render --scripts`.
7. **Write-once uniforms follow their writer.** `BindCache::first_write` was
   true once per cache. A panel's cache wrote its own texel size into the
   pipeline's shared SSAO-blur, volumetric-blur, FXAA and Kuwahara buffers,
   and the window's cache never wrote it back (1,160 pixels moved one level
   after a page). It now answers true whenever another cache wrote that name
   since, through a process-wide last writer per name. One view writes exactly
   as often as before.

## Alternatives considered

- **Every per-frame uniform in per-view slots, one submission.** This keeps
  #139 for any number of panels, but it touches all ~66 write sites, every
  style's own buffers among them. It is the next step if panels ever need it:
  the `ViewSlot` is where those buffers would move. For two or three panels,
  one more submission each costs less than that refactor.
- **A `wgpu::util::StagingBelt`, so writes are ordered inside the encoder.**
  This is the same breadth of change at every write site, and it adds mapped
  staging buffers with their own recall step.
- **A `SceneRenderer` per panel.** Every mesh, texture, pipeline and cache
  would be duplicated, and each would need its own `update_from_world`.
- **Viewport and scissor on one shared target.** Every post pass and style
  draws full-target triangles and reads neighbours, so each would have to
  learn a sub-rectangle and clamp its taps to it. Per-panel targets give each
  panel's chain its true size for free.
- **A `panel` component on a camera entity.** A page is one layout and its
  draw order. A scene table says that in one place, and scripts swap it
  whole.

## Consequences

- A panel costs a whole frame at its size: its shadow pass, its grass and its
  post chain. It is not timed yet (`--gpu-time` times only the frame's last
  panel, whose scopes the timer keeps).
- Motion on twos (#405) is the frame's. The held poses come from the
  `[post_process] motion_hold` the host read, so a panel's own look can't hold
  poses that the frame doesn't. Its post time does follow the panel's config.
- ASCII's `temporal = "reproject"` remembers the last `view_proj` in the
  shared pass, so with two panels it reprojects from the other panel's
  camera. Plain per-cell history works per panel, since its targets are the
  panel's.
- A panel's look is resolved when the layout is set (file reads), never per
  frame. A look that fails to resolve warns and the panel draws with the
  frame's.
- A whole-frame panel with no gutter or border, at the frame's camera and
  look, is byte-identical to the frame. That includes the sRGB round trip
  through `textureLoad` (`tests/panels.rs`).

Book: [Comic Panels](../book/src/concepts/comic-panels.md).
