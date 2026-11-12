# ADR 0076: Emissive materials

Status: accepted, 2026-10-16 ([#43](https://github.com/chrischaps/Flint/pull/43)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #43's description,
commit `fbab21f`, and PR #57.
Amended by [#57](https://github.com/chrischaps/Flint/pull/57) (glTF
`emissiveTexture`, no new ADR) and by
[ADR 0090](0090-emission-scale-after-upload.md) (scale after upload).

## Context

Games need surfaces that are light sources in their own right: lamps,
windows, embers, signage. Flint's PBR shader had no emissive term:
`fs_main` returned `ambient + Lo`. The Capitol worked round it by shading
glowing faces with a normal pointing straight down, with vertex colours
divided by the scene's ground ambient. That tied a light's brightness to
unrelated scene parameters, lost the surface's normal, and broke whenever
the ambient or `diffuse_wrap` changed.

## Decision

1. Emission is a material property, `emissive × emissive_strength`, in
   linear HDR radiance. It's added after all lighting and before
   tonemapping, so bloom picks it up. It doesn't light other surfaces.
   Point lights do that.
2. For vertex-coloured runtime meshes there's a per-vertex form. Vertex
   alpha is the surface/light mask: 1 is surface, 0 is light. Alpha 1 means
   "surface" so that every existing vertex colour is unchanged when the
   flag is on. Vertex alpha stops meaning opacity, so the mode is opt-in,
   per material (`vertex_emission`).
3. glTF `emissiveFactor` and `KHR_materials_emissive_strength` map straight
   onto the fields.

`ImportedMaterial` gains `emissive`, `emissive_strength` and
`vertex_emission`, and an `impl Default`; the engine's full struct literals
end in `..Default::default()`. `MaterialUniforms` gains `emissive: [f32; 4]`
(rgb × strength, w the per-vertex scale), filled by `set_emission`.

## Alternatives considered

- A fifth vertex attribute for emission. That would change every pipeline's
  vertex layout, and every mesh would pay 16 bytes per vertex for a rarely
  used feature.
- Encoding emission in UVs. Untextured meshes don't use them, but that
  would be invisible and fragile.
- Overbright vertex colours under a down normal (the old workaround). It
  depends on the ambient and breaks with diffuse wrap.

## Consequences

- `MaterialUniforms` grows by 16 bytes (64 to 80). Any game that builds
  `ImportedMaterial` with a full struct literal must add the three fields
  or `..Default::default()`.
- Since then: PR #57 read glTF `emissiveTexture` into
  `ImportedMaterial::emissive_texture`. The PBR material group gained
  bindings 7 and 8, `MaterialUniforms` gained `has_emissive_tex` (80 to 96
  bytes), and the shaders multiply the emission by the map's rgb.
  Per-vertex emission ignores the map. It found that glTF data maps were
  decoded as sRGB (fixed in #61).
- Since then: [ADR 0090](0090-emission-scale-after-upload.md) lets an
  entity scale the emission every frame, and [ADR 0096](0096-glow-surface-after-upload.md) lights a per-vertex
  light part as surface too.

## Measured

`emission_ignores_facing_and_light` (`runtime_mesh.rs`): a black metal
giving off green matches within 3/255 across ambient 0 and 1, an overhead
sun off and on, and the normal facing up or down.

## Sources

- [#43](https://github.com/chrischaps/Flint/pull/43) / commit
  [`fbab21f`](https://github.com/chrischaps/Flint/commit/fbab21f9abaed0cd03fee3e8c1e9615d92c9de26)
  "[H7] Emissive materials: light of its own for any mesh (Flint)"
- [#57](https://github.com/chrischaps/Flint/pull/57) / commit
  [`5f1c0cc`](https://github.com/chrischaps/Flint/commit/5f1c0cc26782e50001176bd3a0a993c639594ed9)
  "[#100] glTF emissive textures: a map that multiplies the emission
  (Flint)"
- Vault: "The Capitol - glTF emissive textures (GH-100) - 2026-10-17"
- Code now: `crates/flint-render/src/pipeline.rs` (`MaterialUniforms`),
  `crates/flint-render/src/shader.wgsl`, `skinned_shader.wgsl`,
  `crates/flint-render/tests/runtime_mesh.rs`; book:
  [Rendering](../book/src/concepts/rendering.md), "Emission"
