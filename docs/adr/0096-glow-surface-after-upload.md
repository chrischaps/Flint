# ADR 0096: A light face's surface share is set per entity, after upload

Status: accepted, 2026-10-20 ([#69](https://github.com/chrischaps/Flint/pull/69)).
Backfilled 2026-11-12 (#374) from PR #69, whose body holds the ADR text,
commit `7115b23`, and the Vault note "2026-10-20 Capitol Gold Paints By Day
And Glows By Night".

## Context

[ADR 0076](0076-emissive-materials.md) made vertex alpha the surface share of a per-vertex light face,
fixed at upload. [ADR 0090](0090-emission-scale-after-upload.md) added `material.emissive_scale` to change the
light's strength each frame, and its consequences named the gap: "a
surface/light split that moves (lit paint by day, light by night) still
needs a rebuild or two meshes". The gold on a player's own plots in The
Capitol does both with the hour, so it was rebuilt and re-uploaded about 40
times a game day (TheCapitol#139, H26).

## Decision

An ECS field, `material.glow_surface` (0 to 1), adds lit surface under a
light part without removing any light. A vertex of alpha `a` is lit as
surface by `a + (1 − a) × glow_surface` of its colour, and gives off
`(1 − a)` of it times the strength, as before.

- Read every frame in extraction, on the entity and then its parent, like
  `emissive_scale` (the two share `ecs_material_f32`). Imported and runtime
  meshes, skinned or not. Clamped to 0..1.
- Folded into the material uniform extraction already builds:
  `MaterialUniforms::glow_surface` takes a pad word, so the struct stays 96
  bytes. Used in `shader.wgsl` and `skinned_shader.wgsl`.
- The default is 0, the mesh as uploaded. Surface vertices (alpha 1) don't
  change, so a mesh can mix plain faces with ones that slide.

## Alternatives considered

- **A mix that moves alpha toward 0 or 1** (the issue's first sketch,
  `emission_mix`). One number that trades surface against light ties the two
  together: paint 1 with any light at all would need an infinite scale, and
  a move toward light would turn plain surface vertices into light too.
- **Per-entity emissive colour** (`material.emissive_r/g/b`). That changes
  the material's own light, not how a per-vertex face splits.
- **Two meshes, one faded with `opacity`.** Two draws, coplanar faces
  fighting for depth, and blending instead of lighting.

## Consequences

- With `emissive_scale`, two numbers per entity give a light face any mix of
  lit paint and light from one upload: (1, 0) is lit paint, (0, 1.6) light
  only.
- A new value rebuilds that draw's material uniform on the next frame, as
  for `emissive_scale`.
- The surface's colour was still the vertex colour, so a face whose hue
  moves with the hour needed a per-entity tint. Since then:
  [ADR 0102](0102-per-entity-vertex-tint.md) added it.

## Sources

- [#69](https://github.com/chrischaps/Flint/pull/69) / commit
  [`7115b23`](https://github.com/chrischaps/Flint/commit/7115b2397c4aaaf1f0a3d9241db39254b5aadb23)
  "[H26] Flint: light a mesh's glow as surface too, after upload (ADR 0096)"
- Vault: "2026-10-20 Capitol Gold Paints By Day And Glows By Night"
- Code now: `crates/flint-render/src/scene_renderer/extract.rs`,
  `crates/flint-render/src/pipeline.rs` (`MaterialUniforms`),
  `crates/flint-render/src/shader.wgsl`, `schemas/components/material.toml`,
  `crates/flint-render/tests/runtime_mesh.rs`
  (`glow_surface_slides_light_to_surface_after_upload`)
- Book: [Rendering](../book/src/concepts/rendering.md), "Lighting the glow as
  surface too"
