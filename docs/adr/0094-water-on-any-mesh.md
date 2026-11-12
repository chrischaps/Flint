# ADR 0094: Water on any mesh, drawn with the ocean's shader

Status: accepted, 2026-10-18 ([#68](https://github.com/chrischaps/Flint/pull/68)).
Backfilled 2026-11-12 (#374) from PR #68, whose body holds the ADR text,
commit `e7465e7`, and the Vault note "The Capitol - water at any height
(GH-200, W7)".

## Context

Flint's ocean is one endless Gerstner sheet at y = 0 on a camera-following
grid. A game can show it through holes in its ground, which works for canals
at sea level. It can't show a lake at its own level, or a river that runs and
falls down a valley. The Capitol's wilds need both: tarns in cirques, rivers
stepping down valleys to the plain (TheCapitol#200, W7).

## Decision

A water body is a mesh drawn with the ocean's own shader.

- `vs_body`, a second vertex entry in `ocean_shader.wgsl`, takes
  `WaterVertex { position, normal, flow }`: the rest surface, its rest normal
  and an XZ flow in m/s. It displaces the mesh with the ocean's wave array,
  scaled by the body's `wave_scale`. The fragment shader is the sea's, so
  waves, Fresnel, refraction, turbidity, the cel ramp, sky reflection and the
  shoreline band ([ADR 0088](0088-shoreline-foam-from-scene-depth.md), [ADR 0091](0091-shore-foam-measured-across-the-water.md))
  carry over.
- Per-body look in group 1, binding 1: deep and shallow colour (`None` takes
  the ocean's this frame, so a time-of-day palette tints bodies too),
  `wave_scale`, `flow_foam`, `flow_period`. The sea binds a body block of its
  own (its colours, full waves, no flow), so one layout serves both pipelines
  and the sea is unchanged.
- Flow is two-phase advection: waves and foam are sampled at `p − flow·age`
  in two phases half a period apart, each fading as it resets, the phase
  staggered by noise. A still body skips the second evaluation, on a uniform
  gate, so the sea pays nothing.
- White water where the flow is fast (from about 0.5 m/s, full by 3.5),
  in streaks along it.
- Falls: `N = normalize(n_wave + n_rest − up)`, so water running down a step
  lights as a slope; for the sea this is the wave normal exactly.
- Drawn after the sea in the same pass, depth-tested and depth-writing,
  through the same grab pass. Several bodies at once.
- API: `ExtensionGpu::set_water_body(id, &WaterBodyMesh, WaterBodyLook)`,
  `set_water_body_look`, `remove_water_body`, `clear_water_bodies` (also on
  `SceneRenderer`), with builders `WaterBodyMesh::lake(outline, level)` and
  `WaterBodyMesh::river(&[RiverPoint])`.
- Scenes: a `water_body` component (`shape = "lake"` or `"river"`), rebuilt
  only when it changes. A scene with bodies and no `ocean` gets the default
  ocean's waves and look.

## Alternatives considered

- **Carve every lake and river down to sea level** (the stopgap): no tarns,
  no falls.
- **A separate water shader for bodies:** two copies of the look to keep in
  step. Sharing the fragment shader keeps one.
- **A flow-map texture per body:** needs a game-side rasteriser; per-vertex
  flow comes free with the mesh.

## Consequences

- Waves move a body's vertices only as finely as it's tessellated; normals
  are per pixel, so the look holds on a coarse lake.
- As decided: `ocean_height` knows only the sea, a lake outline has no holes,
  and bodies aren't frustum-culled or shadow casters.
- The ocean shader's validation became a test
  (`ocean_shader_wgsl_parses_and_validates`), since a failure would otherwise
  only show as the pipeline's `catch_unwind` turning all water off.
- Since then: [ADR 0099](0099-water-body-scales-shore-band.md) gives each body
  a scale on the shoreline band, which this record left to the `ocean`
  component. The demo moved from `demo/` to `examples/` (#763).

## Measured

`demo/shoreline.scene.toml` rendered with main and with this change differed
in 11 of 2,073,600 pixels, by 1/255.

## Sources

- [#68](https://github.com/chrischaps/Flint/pull/68) / commit
  [`e7465e7`](https://github.com/chrischaps/Flint/commit/e7465e71b544373a48cd499d5e6e48622de66387)
  "[W7] Flint: water on any mesh, with flow (ADR 0094)"
- Vault: "The Capitol - water at any height (GH-200, W7) - 2026-10-18"
- Code now: `crates/flint-render/src/water_body.rs`,
  `crates/flint-render/src/ocean_shader.wgsl` (`vs_body`),
  `crates/flint-render/src/ocean_pipeline.rs`,
  `crates/flint-player/src/extension.rs` (`set_water_body`),
  `schemas/components/water_body.toml`, `examples/water_bodies.scene.toml`
- Book: [Ocean](../book/src/concepts/ocean.md), "Water bodies"
