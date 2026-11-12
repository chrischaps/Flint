# ADR 0106: A ground fog lies on floors of its own, from a map the game supplies

Status: accepted, 2026-10-21 ([#82](https://github.com/chrischaps/Flint/pull/82)).
Backfilled 2026-11-12 (#374) from PR #82, whose body holds the ADR text,
commit `dd9d05f`, and the Vault note "2026-10-21 Capitol Mist Lies In The
Valleys As Air". Follows [ADR 0101](0101-height-fog-layer-and-aerial-tint.md).

## Context

Height fog ([ADR 0101](0101-height-fog-layer-and-aerial-tint.md)) is one horizontal layer over the whole scene. Set low
enough to fill a high valley, it buries a plain below; set at the plain, it
misses every valley above it. The Capitol's valley mist lies in hollows at
many heights and never on the city's plain, so it was three translucent
sheets draped over the ground: from above a veil with soft edges, and seen
edge-on from a valley floor, nothing at all (TheCapitol#297).

## Decision

- `flint_render::GroundFog { origin, size, width, depth, texels }`: a map over
  a world rectangle, each texel `[floor, amount]` (a world Y and 0 to 1),
  valued at its cell's middle. Over a texel the density is `amount ×
  ground_fog_density` at the floor, falling by e every `1 /
  ground_fog_falloff` metres above it, holding at the floor's under it.
- `SceneRenderer::set_ground_fog(device, queue, &fog)` uploads it as an
  `Rg16Float` mip chain: each floor over the lowest (so half floats keep
  centimetres over a few hundred metres), the sizes as the GPU halves them,
  and each coarse floor the mean of the fine ones weighted by amount. A bad
  map errs and keeps what was set; a map with no fog clears it.
  `ExtensionGpu::set_ground_fog` / `clear_ground_fog`.
- The look is on `PostProcessConfig`, so it can change every frame:
  `ground_fog_density` (0, the default, is off), `ground_fog_color` (linear,
  unlit), `ground_fog_falloff`, `ground_fog_steps` (1 to 64, default 24).
- The composite marches each view ray through the box the fog can lie in,
  ending where the depth buffer says the ray meets something. Steps lengthen
  with distance from the eye (`(t0+1)·r^i − 1`). The map is read at each
  step's ends at the mip level the step spans; along a step the height over
  the floor runs straight, and the falloff is integrated exactly ([ADR 0101](0101-height-fog-layer-and-aerial-tint.md)'s
  closed form, split where the ray goes under the floor). The ends are
  jittered by a pixel hash, so leftover error reads as grain, not bands.
- Mixed over the scene before the aerial tint and the fog. F3's Fog section
  tunes it.

## Alternatives considered

- **Read the map once mid-step.** Tried: it made a visible grain on slopes,
  because the mist is densest within a metre of its floor and one floor per
  step misses the slope by that much.

## Consequences

- Opt-in: nothing changes without a map and a density over 0; off, the
  shader skips it on a uniform flag. No new dependencies.
- Cost: `steps + 2` texture reads for each pixel whose ray crosses the box.
- Unlit: no shadowing or in-scattering, so the game colours it by the hour.
- One map per renderer.
- The floor is the map's, not the depth buffer's.
- `PostProcessUniforms` grew by four rows, 304 to 368 bytes.

## Measured

In The Capitol at 1080p uncapped, 24 steps: frame times at dawn with the mist
5.4 to 5.6 ms, against 5.2 to 6.3 ms with the old sheets, within the noise.

## Sources

- [#82](https://github.com/chrischaps/Flint/pull/82) / commit
  [`dd9d05f`](https://github.com/chrischaps/Flint/commit/dd9d05f75a45cbd7126799730414bf264a4968d6)
  "[#297] Flint: ground fog on floors of its own (ADR 0106)"
- Vault: "2026-10-21 Capitol Mist Lies In The Valleys As Air"
- Code now: `crates/flint-render/src/ground_fog.rs`,
  `crates/flint-render/src/composite_shader.wgsl`,
  `crates/flint-render/src/postprocess.rs`,
  `crates/flint-render/src/scene_renderer/mod.rs` (`set_ground_fog`),
  `crates/flint-player/src/extension.rs`, `crates/flint-debug-ui/src/render_panel.rs`
- Book: [Post-processing](../book/src/concepts/post-processing.md), "Ground
  fog"; [Player Extensions](../book/src/guides/player-extensions.md)
