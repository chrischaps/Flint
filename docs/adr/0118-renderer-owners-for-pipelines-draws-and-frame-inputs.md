# ADR 0118: The renderer gets one owner each for pipelines, draw kinds and frame inputs

Status: accepted, 2026-11-10 (#736, with #735, #737, #739; audit epic #674)

## Context

CLAUDE.md keeps several rules for the renderer by hand. The audit found a
miss for each (line numbers at `54f1f6ba`):

| Rule kept by hand | The miss |
|---|---|
| "A new scene pipeline must be added to `set_msaa`" | `echo_lighten_pipeline` and both wooze pipeline sets are not rebuilt, and `RenderPipeline::sample_count` is never updated (`scene_renderer/mod.rs:5606-5701`). Changing MSAA at run time in a scene that uses them binds a pipeline with the wrong sample count (#735). |
| "A new transparent or particle draw path needs a veil draw too" | Echoes and billboards are drawn translucent but are not in the style veil (`mod.rs:5397-5438`), so one in front of a styled entity is stylized as that entity (#737). |
| "Hosts call X before Y" | The viewer never calls `set_terrain_entity` or `set_shallow_wooze`, so styled terrain is unmasked in `flint edit`. Open viewer bugs #332, #331, #297 and #474 are each a skipped step (#739). |

Behind the misses is the shape of `SceneRenderer`:

- **171 fields** (`mod.rs:563-872`), and `mod.rs` is 6,713 lines, up from
  1,823 when #4 split it. That split was by phase (`extract.rs`,
  `render_passes.rs`), so each feature since has still added fields,
  methods and a block in the constructor.
- **Two constructors that are copies.** `new` (883-1147) and `new_headless`
  (2426-2692) repeat the pipeline creation and a 170-line struct literal.
- **Nine draw lists walked separately in about ten passes**: shadow, per-frame
  uniforms, main, wireframe, style bounds, style mask and veil, selection,
  stats, and the LOD and frustum culls.
- **Per-frame inputs as loose public fields and setters**: `ocean_time`,
  `style_dt`, `grass_time`, `pixels_per_point`, `scene_dir`, `set_boil`,
  `set_shallow_wooze`, `set_terrain_entity`, `update_particles_from`, and
  per-entity tables the host writes and clears by hand. Five clocks. The
  order hosts must call things in is written only in prose.

## Decision

1. **`ScenePipelines` owns every pipeline that depends on the sample
   count.** `ScenePipelines::build(device, format, samples, layouts)`
   returns the whole set, with lazily built ones as `Option`s that are
   rebuilt when present. Both constructors and `set_msaa` call it and replace
   the struct. `set_msaa` has no per-field copies, so a new pipeline cannot
   be forgotten.

2. **One constructor body.** `SceneRenderer::build(device, queue, format,
   size, config)` sits behind `new` and `new_headless`.

3. **Feature subsystems own their state.** `GrassRenderer` (about 35
   fields), `WaterRenderer` (about 25), `SkyRenderer`, `TerrainRenderer`,
   `SkinningState`, `MsaaTargets`, `LightState` and `StyleMaskState`, each
   with `new(device, layouts, samples)`, its extract step and its draw
   hooks. `SceneRenderer` holds them and orchestrates the frame.

4. **Draws are one list of items that passes filter.**

   ```rust
   struct DrawItem {
       entity: EntityId,
       mesh: MeshRef,
       binds: DrawBinds,
       kind: DrawKind,          // Static, Skinned, Terrain, Billboard, Particles, Echo
       blend: Blend,            // Opaque, AlphaBlend, Additive, Lighten
       vertex: VertexStage,     // Plain, Wooze
       casts_shadow: bool,
       writes_depth: bool,
   }
   ```

   Each pass is a filter and a pipeline selector over the list: the veil
   takes every item that blends and doesn't write depth, the mask takes every
   item whose entity has a style id, and so on. A new kind of draw is one new
   variant, and the compiler asks each pass's selector what to do with it.

5. **Hosts pass inputs in, and bind a scene once.**

   ```rust
   let scene = renderer.bind_scene(SceneBinding { scene_dir, terrain_entity, .. });
   renderer.prepare(&world, &FrameInputs { time, dt, camera, pixels_per_point,
                                           boil, shallow_wooze, particles, world_lines, .. });
   renderer.encode_to(&mut encoder, target, depth);
   renderer.frame_submitted();
   ```

   `bind_scene` loads the scene's user styles, resets style history and
   clears per-entity tables. `FrameInputs` carries one clock. The loose
   public fields become private. The player, the viewer and `flint render`
   call the same three entry points, and a host that leaves a field of
   `FrameInputs` at its default gets the feature off, not a stale value.

## Steps

One agent owns #736 and carries out steps 2 and 3 under its one claim, a PR a
step (or several, where a step says so): `Part of #736` on each PR but the
last, `Closes #736` on the last. The ticket's note has the details. A step
that names another ticket is that ticket's work, and waits its turn through
the tickets' blocked-by links. The steps get no sub-issues.

1. `ScenePipelines` and the `set_msaa` fix, with a GPU test that switches
   1 to 4 to 1 with every draw kind on screen (#735). A bug fix; it can land
   before the ADR is accepted.
2. The single constructor body (#736, first part).
3. Subsystems, one per PR, smallest first: `MsaaTargets`, `SkyRenderer`,
   `LightState`, `TerrainRenderer`, `StyleMaskState`, `WaterRenderer`,
   `SkinningState`, `GrassRenderer` (#736).
4. `FrameInputs` and `bind_scene`, with the viewer moved onto them (#739).
5. `DrawItem`, and the veil cases for echoes and billboards as goldens
   (#737).

## Alternatives considered

- **A render graph** (passes declare inputs and outputs; the graph orders
  and allocates). It would make the pass order data and remove hand-kept
  target plumbing. It is a rewrite of a renderer that works, the frame's
  order is fixed and well understood, and the stylize framework already
  gives the one place that needed data-driven order. Not now.
- **Only a test per rule** (a test that draws everything after `set_msaa`, a
  golden per veil case). Cheaper, and worth having anyway, but it catches a
  miss only for the draw kinds someone remembered to put in the test.
- **Split `mod.rs` by file only.** Moves lines and leaves 171 fields on one
  struct, which is what #4 did.

## Consequences

- **Byte-identical output is the gate** for every step: the 74 stylize
  goldens, `render_time_cli`, and the frame captures. Steps 2 and 3 are pure
  moves and should change no pixel. Step 5 changes pixels only where the
  veil was wrong.
- **`DrawItem` and performance.** The extract step rebuilds draw lists every
  frame, and CLAUDE.md records how a 270-entity scene went from 17 ms to
  4 ms when per-entity allocation was removed. One list must stay
  allocation-free per item (items hold cache handles, not owned buffers) and
  be sorted once. Measure `--gpu-time` and CPU frame phases on the woozy
  grove and a 270-entity scene before and after, and count WebGPU calls per
  frame on the web (`?perf=1`).
- **Public API changes** for anything embedding the renderer: the loose
  fields go, and pipeline and GPU uniform types become `pub(crate)`. In this
  repo that is the player, the viewer and the CLI. A game that drives the
  renderer directly through a `PlayerExtension` uses `ExtensionCtx`, which
  is unaffected.
- Subsystems make the per-feature cost visible: a scene with no grass
  constructs no `GrassRenderer` state beyond an empty struct.

## Decisions

Chris accepted the recommendations on 2026-11-10:

1. **The order stands**: the `set_msaa` fix, one constructor, subsystems,
   frame inputs, then draw items.
2. **`flint_render`'s surface narrows with no deprecation period**: what the
   player, viewer and CLI don't use becomes `pub(crate)`, after a check that
   Float, FlintKart and Kandinski import none of it (before step 4).
3. **No render graph.** The subsystems' draw hooks are not shaped as passes.
