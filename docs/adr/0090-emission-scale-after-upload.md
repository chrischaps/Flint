# ADR 0090: Emission scale per entity, after upload

Status: accepted, 2026-10-18 ([#62](https://github.com/chrischaps/Flint/pull/62)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #62's description and
commit `a51fa83`.
Extended by [ADR 0096](0096-glow-surface-after-upload.md) (`material.glow_surface`).

## Context

[ADR 0076](0076-emissive-materials.md) put emission in the material uniform
at upload (`ImportedMaterial::emissive`, `emissive_strength`,
`vertex_emission`). Nothing could change it afterwards. A game whose lights
follow the time of day had to rebuild its meshes on every change. The
Capitol's house windows are 16 merged meshes holding up to about 75k panes,
rebuilt and re-uploaded each time the evening light stepped.

## Decision

An ECS field, `material.emissive_scale`, multiplies the uploaded emission:
the material's own `emissive` (and its map) and per-vertex emission alike.
Extraction reads it each frame and folds it into the material uniform that
is already built there (`opacity`, `texture_scale` and the tint work the
same way). Default 1, negative counts as 0. It applies to imported and
runtime meshes, skinned or not, and a child with no scale of its own takes
its parent's.

Per-vertex emission gets its own flag, `MaterialUniforms::vertex_emission`
(from a pad word, still 96 bytes). The shaders branch on it rather than on
`emissive.w > 0`, so the strength can reach 0 without changing what vertex
alpha means: at 0 a light vertex is an unlit bulb, black, not a lit,
see-through surface.

## Alternatives considered

- `ExtensionGpu::set_emissive_strength(mesh, s)`: per mesh, not per entity.
  Two entities sharing a mesh couldn't differ, scripts couldn't reach it,
  and it would be a second source of truth beside the material component.
- Rewriting only the uniform buffer in place: the draw-bind cache rebuilds
  a draw's material bind group when its uniform bytes change, as it does
  for `opacity`. That costs a 96-byte buffer and a bind group per changed
  draw, cheap at the rates games change light. An in-place `write_buffer`
  path is a possible follow-up if something animates hundreds of glows per
  frame.

## Consequences

- One number per entity changes a glow.
- A surface/light split that moves (lit paint by day, light by night)
  still needed a rebuild or two meshes, because vertex alpha is fixed at
  upload. Since then: [ADR 0096](0096-glow-surface-after-upload.md) added `material.glow_surface`, which lights a
  per-vertex light part as surface too, so with `emissive_scale` one upload
  slides from lit paint to light.
- Behaviour change, small: an uploaded material with
  `vertex_emission: true` and `emissive_strength: 0` used to fall back to
  vertex alpha as opacity. Now its light vertices render black.

## Measured

In The Capitol, windows uploaded once and lit by `emissive_scale` match the
old rebuild pixel for pixel at midnight and noon, apart from the stars'
twinkle. Evening differs by less than 24/255 on fewer than 25 pixels: 16
extra draws, the same triangles, and no frame-time change at vsync.

## Sources

- [#62](https://github.com/chrischaps/Flint/pull/62) / commit
  [`a51fa83`](https://github.com/chrischaps/Flint/commit/a51fa830a54965550624d56b829f52b02bd70ed0)
  "Change a mesh's emission after upload: material.emissive_scale (ADR
  0090)"
- Commit
  [`7115b23`](https://github.com/chrischaps/Flint/commit/7115b2397c4aaaf1f0a3d9241db39254b5aadb23)
  "[H26] Flint: light a mesh's glow as surface too, after upload (ADR
  0096)"
- Vault: "The Capitol - emission after upload (H23, GH-130) - 2026-10-18",
  "2026-10-20 Capitol Gold Paints By Day And Glows By Night"
- Code now: `crates/flint-render/src/pipeline.rs` (`MaterialUniforms`,
  `scale_emission`), `crates/flint-render/src/scene_renderer/extract.rs`,
  `schemas/components/material.toml`; book:
  [Rendering](../book/src/concepts/rendering.md), "Changing the glow after
  upload"
