# ADR 0125: Translucent draws are fogged at their own depth, through a fog veil

Status: accepted, 2026-11-16 (#513).

## Context

Depth fog (distance and height fog, the aerial tint, ground fog) is drawn in
the composite from the depth buffer ([ADR 0078](0078-fog-reaches-everything-drawn.md),
[ADR 0101](0101-height-fog-layer-and-aerial-tint.md),
[ADR 0106](0106-ground-fog-on-own-floors.md)). Translucent draws (blended
static and skinned meshes, living props' echoes, particles) write no depth,
so each of their pixels was fogged by the depth of whatever lay behind it.
Where that depth jumps, the translucent surface showed a hard line.
chrischaps/Embodiment#3 found it: a living prop's echoes 3 m from the camera,
fog density 0.09 from 4 m, an 80 m floor. Above the floor's far edge the
echoes were fogged as sky (not at all), below it as far floor (almost wholly),
so the echoes stopped at the horizon.

A pixel holding a translucent layer over what's behind it is, in linear light,
`C = t B + S`: `t` the share of the background `B` that still shows, `S` the
translucent draws' own light. Fogging it right fogs the two apart:
`W = fog_b(t B) + fog_t(S)`, `b` the scene's depth there and `t` the
translucent draw's. The composite had `C` and `b` only. No single fog factor
on `C` gives `W`: the error is of order `a (1 - a) (fog_b - fog_t)(B - T)`,
which is the seam. So the composite needs, per pixel, the background apart
from the rest (`B`), how much of it shows (`t`), and the translucent depth.

## Decision

While the fog (or ground fog) is on and the camera sees a draw that blends,
the frame keeps three things and the composite fogs each veiled pixel as two
layers (`scene_renderer/fog_veil.rs`):

- **The picture behind.** The main pass ends after the opaque scene and the
  ocean (a third segment of `RenderPhase`, next to the ocean grab's split),
  the resolved HDR buffer is copied (`copy_texture_to_texture`; the HDR
  texture is now `COPY_SRC`), and the pass goes on with the 2D sprites, the
  blended draws, the echoes, the particles and the overlays.
- **The fog veil.** After the scene passes, every drawn item whose `Blend`
  blends is drawn again into an `Rgba16Float` target against the scene's
  single-sample depth (read-only, as the style veil, #290), with new
  `fs_fog_veil` entry points in `shader.wgsl`, `skinned_shader.wgsl` and
  `particle_shader.wgsl` that write `(w (1 - z), w, 0, w)`: `w` the alpha
  the main pass drew with (`veil_alpha`, which `fs_veil` now calls too), `z`
  the fragment's depth. "Over" blended on the colour channels and
  `a' = a (1 - src.a)` on alpha, cleared to `(0, 0, 0, 1)`: `r / g` is the
  layers' weighted `1 - depth` (near layers hide far ones' weight as they
  hide their colour) and `a` is `t`. A draw that only adds light (additive
  meshes and particles, an echo's `lighten`) writes alpha 0 through
  `fs_fog_veil_light`: it weighs in and covers nothing. Echoes draw front
  faces only, as in the main pass; a skinned body translucent only at its soft
  edge writes depth already and is left out.
- **A second composite.** `COMPOSITE_VEILED_WGSL` is the same file with
  `const FOG_VEILED: bool = true;` prepended (the plain one gets `false`, so
  frames without the veil draw from code that folds the path out), built the
  first frame that needs it. Where the veil has weight, the pixel's own light
  is `own = C - t B` and the rest of the pixel (`color - own`, which keeps
  SSAO, bloom and god rays with the background) is the background. Each goes
  through `fog_layer` (ground fog, tint, fog, each fog colour scaled by the
  layer's share of the pixel) at its own depth, `b` and `1 - r / g`, and the
  two are added. Elsewhere the fog is the old code, unchanged.

The composite reads the veil only when it reads the HDR buffer itself: an HDR
style's result isn't the picture the veil and the copy were taken over.

## Alternatives considered

- **Fog translucent draws in their own shaders and have the composite skip
  the pixels they cover** (the issue's first idea, what forward renderers do).
  Skipping the pixel also skips the background's fog, so a faint echo would
  show the unfogged floor through it. It works in forward renderers because
  they fog the opaque draws in their shaders too; here that would mean
  moving all fog (ground fog's ray march included) into every translucent
  shader, and the composite's order (SSAO, bloom, god rays, then fog) would
  change for every fogged frame.
- **One depth for the whole pixel** (min of the opaque and an echo-only
  depth, the issue's stopgap). It fogs the background at the translucent's
  depth: a hole in the fog the shape of every faint echo.
- **Carry `t` in the HDR buffer's alpha**, with an alpha clear before the
  translucent draws. It needs every translucent pipeline's alpha blend
  changed (additive and lighten add alpha today), still needs the depth from
  somewhere, and the alpha reaches the surface when there's no post chain.
  The veil pass gets both from one redraw of vertex work and a tiny fragment.
- **Fog the opaque picture between the two halves of the pass**, then let
  translucent draws fog themselves. Exact for the translucent layer, but it
  moves the fog ahead of SSAO, bloom and god rays only on frames with a
  translucent draw, so a particle coming into view would change the look of
  the whole frame.

## Consequences

- A frame with no fog, or no translucent draw in view, records exactly what it
  did: one pass (or the ocean's two), no copy, no veil, the plain composite.
  Against main's binary, 122 of the 129 golden renders are byte-identical,
  and so are A/B renders of fogged scenes with no translucent draw in view
  (`woozy_grove`, `volumetric_test`, `terrain_test`, `showcase_crucible`,
  `sweep_garden`, `living_compare`, `pbr_test`) and of `style_veil`
  (translucent draws, no fog). The seven that move are the selection cases,
  whose fixture has fog and a glass box: the glass moves by at most 6 levels
  (about 5,300 of 230,400 pixels), within the goldens' tolerance.
- Several translucent layers are fogged as one, at their weighted depth (the
  nearest weighs most). A woozy translucent draw goes into the veil through
  the plain vertex stage; fog changes slowly with depth, so the few
  centimetres don't show.
- The veil is single-sample, like the style veil; under MSAA its edges are a
  pixel's.
- A new kind of draw is asked what it veils: `fog_veil::veils` matches every
  `Draw` variant.
- Cost while drawn: the HDR copy, one more pass break (a resolve and reload of
  the MSAA targets under MSAA), the translucent draws' vertex work again with a
  tiny fragment, and on veiled pixels the fog twice more (ground fog's march
  included).

## Measured

`flint-examples/fog_veil.scene.toml` at 1280x720, `flint render --time 2 --gpu-time
200`, RTX 4070 on Vulkan, debug build, main's binary and this branch's run
alternately three times, the least p50 of each pass:

| Pass | Main | This branch |
|---|---|---|
| Scene (with the split and the copy) | 0.209 ms | 0.226 ms |
| Fog veil | | 0.054 ms |
| Composite | 0.016 ms | 0.025 ms |
| Sum | 0.225 ms | 0.305 ms |

About 0.08 ms a frame while the veil is drawn; nothing on a frame without it.

Book: [Post-Processing, Fog](../book/src/concepts/post-processing.md#fog).
Bench: `flint-examples/fog_veil.scene.toml`. Test:
`crates/flint-render/tests/runtime_mesh.rs::a_translucent_draw_is_fogged_at_its_own_depth`.
