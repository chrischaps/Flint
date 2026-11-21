# ADR 0132: One point light's shadow, as a cube atlas in one layer

Status: accepted, 2026-11-21 (#1112).

## Context

ADR 0124 (#504) gave one spot light a shadow: `cast_shadows = true`, a
perspective map in layer 3 of the cascades' texture, PCSS from
`source_radius`. Point lights still lit without one, and a point light that
asked logged `shadow-cap` ("a point light casts no shadow yet"). The case
it leaves open is the commonest lamp in the examples: a lantern on a table,
a candle in a room, light that falls every way at once. A spot can't stand
in for one: whatever its cone leaves out goes unlit.

A point light sees the whole sphere around it, so its shadow is a cube of
six perspective maps, one down each axis. Every lit shader reads shadows
through group 2 (`shadow_maps`, a `texture_depth_2d_array`, its comparison
sampler and `ShadowUniforms`), shared by six WGSL mirrors; ADR 0124 kept that
binding by putting the spot in a layer of the array.

## Decision

- **A point light opts in with `light.cast_shadows = true`**, as a spot
  does. One point light casts: the first by entity name that asks
  (`PointLight::shadow = 1.0`, the former `_pad0`). Another that asks lights
  without one and is said once (`shadow-cap`, naming the one that casts).
  The budget is per kind, not shared: one spot and one point light may both
  cast. Unset is false, so every existing scene is unchanged.
- **The six faces are one more layer of the cascades' texture**
  (`POINT_SHADOW_LAYER` = 4, `SHADOW_LAYERS` = 5), laid out as an atlas: three
  faces across and two down, each `resolution / 3` texels wide (682 at the
  default 2048), drawn in one render pass with a viewport and scissor a face.
  Face order +X, -X, +Y, -Y, +Z, -Z; face `f` at column `f % 3`, row `f / 3`.
  No binding, layout or bind group changes.
- **`ShadowUniforms` grows at its end** by the six faces' view-projections, a
  `vec4` (near, far, tan of a face's half-angle, drawn) and a `vec4` (a
  face's width as a share of the layer, in texels). 304 to 720 bytes. The
  shader picks the face by the dominant axis of the direction from the lamp
  and reads it through its own matrix, so the faces' up vectors are a CPU
  choice the shader never repeats.
- **Each face reaches past 45 degrees** (`POINT_FACE_TAN_HALF` = 1.08, about
  47.2 degrees): a 25-texel band past the face's edge at 682 texels. The
  PCSS kernel is capped at 24 texels of a face, so near a seam a tap still
  finds real depths on the face the lookup chose, and every tap is clamped
  inside that face's square of the atlas (half a texel in) as well, so no
  tap reads a neighbour's.
- **Softness from `source_radius`**, the spot's PCSS in a face's terms:
  linear depth along the face's axis, a 16-tap blocker search sized by the
  lamp's disc at the near plane, a 32-tap filter of radius
  `R (z_r - z_b) / z_b`, the disk turned per pixel by interleaved gradient
  noise, each tap held under the receiver's plane by its slope toward the
  lamp. `source_radius = 0` is a 3x3 PCF. Near plane, far plane and normal
  offset follow the spot's (`range * 0.004` clamped to 0.02-0.25 m, `range`,
  1.5 texels at the receiver's depth).
- **Called only where the light gives light** (`atten > 0`), in
  `direct_light.wgsl` (meshes, skinned meshes, terrain) and in
  `local_lights.wgsl`'s `point_light_at` (grass and water), which now takes
  the receiver's normal and pixel as `spot_light_at` does.
- **Drawn with or without a sun or a casting spot.** `point_shadow_drawn()`
  is new; `shadow_layers_drawn()` counts the layer once, and the stats count
  six views of draws for it.

## Alternatives considered

- **Six layers of the array, one a face.** The simplest lookup (layer =
  face) and full-resolution faces, but every layer of the array is the
  cascades' size: six more at 2048 is 96 MB, allocated in every scene.
- **A `texture_depth_cube`.** Hardware seams and filtering for free, but a
  new group 2 binding in six WGSL mirrors and every bind group that builds
  them; rejected for the same churn ADR 0124 avoided.
- **A layer allocated only when a scene asks.** Saves the 16 MB in scenes
  without a casting point light, at the cost of remaking the pass and the
  light bind group when a script turns `cast_shadows` on. Still left for
  later, for both layers (ADR 0124).
- **One caster shared between spot and point ("one light casts").** Would
  have reused layer 3 and cost no memory, but a scene with a key spot and a
  lantern would have had to choose.
- **Kernel taps sampled across seams onto the neighbouring face.** Exact,
  but each tap would need its own face choice and matrix; the guard band
  gives the same answer for a kernel that fits inside it.

## Consequences

- A scene with no point light asking draws the bytes it drew before: every
  stylize golden and every example scene render identical PNGs against main's
  binary (#1112's PR).
- The shadow texture holds one more layer whatever the scene (16 MB at the
  default 2048 texels, 4 MB at 1024). The cube's faces are a third of the
  cascades' resolution.
- A scene whose point light casts draws its casters up to six times more
  (once a face they fall in) in one pass, and filters the pixels inside the
  light's range. The lantern-on-a-table bench at 1920 x 1080 on an RTX 4070,
  debug, p50 of `--gpu-time 240`, two alternated runs each: depth pass
  0.011 ms; scene pass 0.089 ms without, 0.518 ms with. About 0.43 ms a
  frame, more than the spot's 0.26, since the whole frame is inside the
  lamp's range and most of it in the table's shadow, where the full 48 taps
  run.

## Sources

- Issue #1112; follows #504.
- Code: `crates/flint-render/src/shadow.rs` (`POINT_SHADOW_LAYER`,
  `POINT_FACE_TAN_HALF`, `point_shadow_cube`, `ShadowPass::update_point`),
  `scene_renderer/lights.rs` (`gather`, `casts_first`, `casting_point`),
  `scene_renderer/render_passes.rs` (`ShadowLayerDraw`,
  `render_shadow_pass`), `shadows.wgsl` (`point_face`,
  `point_shadow_factor`), `direct_light.wgsl`, `local_lights.wgsl`.
- Book: [Lighting](../book/src/concepts/lighting.md), "Point light shadows".
