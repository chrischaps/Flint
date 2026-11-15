# ADR 0044: `diffuse_wrap` softens the diffuse terminator, carried in `ambient_sky.w`

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0` and its diff, Vault note
"Clay as a Material for Feelings - 2026-08-19", and the code comment citing
it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

In the clay-look spike ([ADR 0042](0042-clay-look-research-spike.md)), Lambert's sharp terminator cut light off
where clay should let it sink past the edge (Vault). The PBR shader had no
lever for it.

## Decision

From the commit's diff:

- `[environment] diffuse_wrap` (f32; absent is 0). The shader replaces the
  diffuse `n·l` with `max((n·l + wrap) / (1 + wrap), 0)` for directional,
  point and spot lights. Specular keeps the true `n·l`.
- `wrap = 0` takes the original code path exactly.
- No uniform field was added. The value rides `ambient_sky.w`, encoded as
  `1 + wrap`, so every existing CPU write of 1.0 decodes to 0 and a stale or
  default uniform still means off.
- `SceneRenderer::set_diffuse_wrap(wrap)` clamps at 0; `reset_ambient()`
  clears it with the ambient ([ADR 0043](0043-scene-authored-hemisphere-ambient.md)). The player and `flint render`
  apply it from the scene.
- The commit's comment calls it "a cheap subsurface-ish cue for matte
  materials"; the book suggests 0.2-0.5.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Since then: [ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md) added Oren-Nayar on `ambient_ground.w` with the same
  `1 + value` encoding, and a Charlie sheen. The three are orthogonal: wrap
  replaces the diffuse `n·l`, Oren-Nayar scales it, sheen adds a rim. With
  all three at zero the shader takes the original path. The F4 menu's
  Lighting section and `lighting_levers()` expose the wrap.

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs [0042](0042-clay-look-research-spike.md)-[0047](0047-viewer-dof-follow.md))"
- Vault: "Clay as a Material for Feelings - 2026-08-19"
- Code now: `crates/flint-render/src/shader.wgsl` (`evaluate_light`),
  `crates/flint-render/src/pipeline.rs` (`LightUniforms::ambient_sky`),
  `crates/flint-render/src/scene_renderer/mod.rs` (`set_diffuse_wrap`),
  [Lighting](../book/src/concepts/lighting.md)
