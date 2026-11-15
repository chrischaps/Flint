# ADR 0102: A vertex-coloured mesh takes a per-entity tint

Status: accepted, 2026-10-21 ([#75](https://github.com/chrischaps/Flint/pull/75)).
Backfilled 2026-11-12 (#374) from PR #75, whose body holds the ADR text,
commit `86892cd`, and the Vault note "2026-10-21 Capitol Windows Turn From
Glass To Lamp". Completes [ADR 0096](0096-glow-surface-after-upload.md).

## Context

[ADR 0076](0076-emissive-materials.md) made vertex colour the albedo of a `use_vertex_color` mesh, and
vertex alpha its surface share. ADRs [0090](0090-emission-scale-after-upload.md) and [0096](0096-glow-surface-after-upload.md) let an entity scale the
light and the surface share after upload. [ADR 0096](0096-glow-surface-after-upload.md)'s consequences named the
gap left: a face whose hue moves with the hour (glass by day, lamp-coloured
by night) would need a per-entity tint of vertex colour. With
`use_vertex_color` the shader ignores the entity's `material.base_color_*`
(TheCapitol#269, H28).

## Decision

A separate ECS field, `material.vertex_tint_r/g/b`, multiplies every vertex
colour before it becomes albedo or per-vertex light.

- All three or none; none is white, the mesh as uploaded. Negatives count as
  0, values over 1 brighten.
- Read every frame in extraction, on the entity and then its parent, like
  `emissive_scale` and `glow_surface`. A channel missing falls back to the
  parent's, then white. Imported and runtime meshes, skinned or not.
- `MaterialUniforms::vertex_tint` is a `vec4` at the end of the struct (96 to
  112 bytes), in `shader.wgsl` and `skinned_shader.wgsl`.
- The schema fields have no default, so the scene loader doesn't fill in
  white on a child and block its parent's tint.

## Alternatives considered

- **Multiply by `base_color`, as glTF multiplies `COLOR_0` by
  `baseColorFactor`.** That would move existing scenes: procedural meshes and
  `spline_gen`'s striped ones upload their vertices in the material's colour
  with `base_color` set to the same colour, so they would be squared. A
  script's `set_material_color` on a vertex-coloured entity, a no-op, would
  start to matter. And glTF `COLOR_0` wasn't read at all, so there was no
  spec behaviour to match.
- **Pack the tint into `base_color` only for vertex-coloured draws.** Saves
  16 bytes but gives one uniform two meanings, and still needs a new field to
  say which.
- **A per-entity emissive colour.** Colours only the light, not the lit
  surface: a pane would be lamp-coloured light on glass-coloured paint.

## Consequences

- With `glow_surface` and `emissive_scale`, a face uploaded once as white
  light can take any colour, lit-paint share and light each frame.
- The material uniform grows by 16 bytes per draw. A new value rebuilds that
  draw's material uniform on the next frame.
- No behaviour change: unset is white.
- The Capitol's house windows upload once as white light and take
  `mix(GLASS, LAMP, R/(S+R))` each frame.

## Sources

- [#75](https://github.com/chrischaps/Flint/pull/75) / commit
  [`86892cd`](https://github.com/chrischaps/Flint/commit/86892cdc0a463082b9822e168beab4e98ec3a062)
  "[H28] Flint: tint a vertex-coloured mesh per entity (ADR 0102)"
- Vault: "2026-10-21 Capitol Windows Turn From Glass To Lamp"
- Code now: `crates/flint-render/src/scene_renderer/extract.rs`,
  `crates/flint-render/src/pipeline.rs` (`MaterialUniforms::vertex_tint`),
  `crates/flint-render/src/shader.wgsl`, `schemas/components/material.toml`,
  `crates/flint-render/tests/runtime_mesh.rs`
  (`vertex_tint_colours_light_and_surface_after_upload`)
- Book: [Rendering](../book/src/concepts/rendering.md), "Tinting vertex
  colours per entity"
