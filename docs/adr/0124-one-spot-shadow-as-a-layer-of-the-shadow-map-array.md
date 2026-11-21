# ADR 0124: One spot light's shadow, as a layer of the shadow map array

Status: accepted, 2026-11-16 (#504).

## Context

Only the strongest directional light cast shadows (cascaded shadow maps,
`i == 0u` in the shaders' directional loop). Point and spot lights lit a
surface and never shadowed it. The case that hurt was a small lit set: one
key light over one subject on a dark floor (chrischaps/Embodiment's stage).
The subject got no contact shadow from the key and read as floating, and the
only workaround, a directional light along the key, flooded the floor and
erased the pool the key made.

Every lit shader reads the shadow maps through group 2 (`shadow_maps`, a
`texture_depth_2d_array`, its comparison sampler and `ShadowUniforms`), and
group 2's layout is shared by six WGSL mirrors (shader, skinned, terrain,
grass, volumetric, ocean).

## Decision

- **A spot light opts in with `light.cast_shadows = true`.** One spot casts:
  the first by entity name that asks. Another that asks lights without one
  and is said once (`shadow-cap`, a new diagnostic code). A point light that
  asks, and the strongest directional light saying `false`, are said too:
  neither is honoured. Unset is false, so every existing scene is unchanged.
- **The spot's map is one more layer of the cascades' texture**
  (`SPOT_SHADOW_LAYER` = 3, `SHADOW_LAYERS` = 4), not a texture of its own.
  No binding, layout or bind group changes: the shaders that draw spot light
  index layer 3 of the array they already sample.
- **`ShadowUniforms` grows at its end** by the spot's view-projection and a
  `vec4` (near, far, tan of the half-angle, drawn). The main, skinned and
  terrain shaders declare the longer struct; grass, ocean and volumetric
  keep the shorter one, which a larger buffer allows (`min_binding_size` is
  unset). `ShadowUniforms` went from 224 to 304 bytes.
- **The chosen spot is marked in its `SpotLight`**: the former `_pad1` slot
  is `shadow`, 1.0 on the spot whose map is drawn. The marking rides the
  light through the camera's choice of the nearest spots (#682), so the
  shader finds it by its slot, wherever that lands; a spot left out by the
  cap has no shadow that frame.
- **The map**: a square perspective from the light down its axis, near at
  `range * 0.004` (clamped to 0.02-0.25 m), far at `range`, half-angle the
  outer cone plus a margin for the filter, capped at 80 degrees. Up is world
  +Y, or +Z when the axis is within about 8 degrees of vertical. Drawn by the
  same pass and pipelines as the cascades, with the same per-layer frustum
  cull and the same `material.cast_shadows`. Grass doesn't cast into it.
- **Softness from `source_radius`** (PCSS, as ADR 0057 does for the sun with
  `angular_size`): a 16-tap blocker search sized by the lamp's disc seen from
  the receiver, then a 32-tap filter of radius
  `R (z_receiver - z_blocker) / z_blocker` metres over the map's width at the
  receiver, both capped at 48 texels. Depths are made linear from the
  perspective map before the ratio. The Vogel disk turns per pixel by
  interleaved gradient noise, so the penumbra is grain rather than rings.
  `source_radius = 0` is a 3x3 PCF.
- **Drawn with or without a sun.** When only the spot casts, the cascades'
  uniforms read "off" (far plane 0) and only layer 3 is drawn.
  `shadows_drawn()` keeps its meaning (the sun's cascades: grass culling,
  volumetrics and the stats read it); `spot_shadow_drawn()` and
  `shadow_layers_drawn()` are new.

## Alternatives considered

- **A texture and binding of its own.** Cleaner to read, but every group 2
  layout, bind group and six WGSL mirrors change, and a 2D depth texture
  needs a second comparison-capable binding where the array already has one.
  Rejected for the churn.
- **A layer allocated only when a scene asks.** Saves 16 MB at 2048 texels
  in scenes without a casting spot, but the pass and the light bind group
  would be remade when a spot starts or stops casting (a script can set
  `cast_shadows`), as a resolution change remakes them. Left for later: the
  quality tiers' lower resolutions already shrink it with the cascades.
- **Several spot shadows.** More layers and a layer index in each
  `SpotLight` would do it; the issue asked for one, and one keeps the cost
  of a scene that asks to one depth pass.
- **Point lights (cube maps).** Six faces, a cube or six-layer view and a
  different lookup; a later step (#1112), taken in ADR 0132 as a six-face
  atlas in one more layer.

## Consequences

- A scene with no spot asking draws the bytes it drew before: every stylize
  golden and every example scene render identical PNGs against main's
  binary (#504's PR).
- The shadow texture holds one more layer whatever the scene (16 MB at the
  default 2048, 4 MB at 1024).
- A scene whose spot casts draws one more depth pass over the casters in its
  view and filters the pixels the spot lights (skipped outside the cone and
  past `range`, where the light adds nothing). `flint-examples/spot_shadow.scene.toml` at
  1920 x 1080 on an RTX 4070, debug, p50 of `--gpu-time 240`: depth pass
  0.024 ms; scene pass 0.075 ms without, 0.316 ms with (0.43 ms before the
  skip). About 0.26 ms a frame.

## Sources

- Issue #504; Embodiment#2 found it.
- Code: `crates/flint-render/src/shadow.rs` (`SPOT_SHADOW_LAYER`,
  `spot_shadow_frustum`, `ShadowPass::update_spot`),
  `crates/flint-render/src/scene_renderer/lights.rs` (`gather`,
  `casting_spot`), `scene_renderer/render_passes.rs` (`render_shadow_pass`),
  `spot_shadow_factor` in `shader.wgsl`, `skinned_shader.wgsl`,
  `terrain_shader.wgsl`.
- Book: [Lighting](../book/src/concepts/lighting.md), "Spot light shadows".
