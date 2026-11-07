# ADR 0111: Stylized rendering — one screen grid, per-entity selection, a pass framework

*Accepted 2026-11-06. Epic #260; decided by the spike #261 and built in #262-#279.
Book: [Stylized Rendering](../book/src/guides/stylized-rendering.md) (the guide) and
[Stylize Passes](../book/src/concepts/stylize-passes.md) (the reference).
Spike write-up: [docs/design/ascii-spike.md](../design/ascii-spike.md).*

## Context

Before the epic, Flint's non-photoreal looks were whole-screen only:
Kuwahara as a fixed pass, and render modes 1-5 as branches inside
`composite_shader.wgsl`. Each new look meant new code in the renderer's
core.

The request was an ASCII look, ideally **per entity**, because glyphs on
chosen objects should give those objects definition. The worry was that
per-entity glyphs would not sit on one consistent grid. ASCII was also the
first of several looks (watercolor #272, pencil #273, user-written WGSL
#274/#275), so the decision had to cover how *any* style plugs in, not
only ASCII.

Three questions:

1. **Full screen or per entity?** And if per entity, whose grid?
2. **Where in the frame do styles run?** In linear HDR before fog, bloom and
   tonemapping, or on the finished picture?
3. **How does a style plug in** without touching the renderer core,
   the scene format, the F4 menu or the CLI?

## Decision

### 1. Per-entity *selection* on one screen grid; full screen is its "everything" case

There is one glyph field: a single grid, centred on the frame. A style can
apply to the whole frame (`[post_process.styles.ascii]`, `scope = "screen"`)
or only to entities that ask for it (`ascii_style` / `style_mask`
components, or `scope = "entities"`). Per entity selects *which pixels*
the style touches. It does not move the grid.

Each styled entity can still opt in to its own grid (`grid = "object"`,
anchored at its projected origin). This is never the default.

Why, from the spike's renders (#261, findings 1-3):

- **The seams come from the object grid, not from per-entity.** With
  every entity selected on the screen grid, the frame was nearly
  indistinguishable from full screen. With object grids, rows started at
  different offsets wherever two styled things met (the tavern's wall and
  floor, the bench and the ground).
- **The object grid buys less than it looks.** It holds glyph phase under
  translation only. Rotation and changes of depth still swim. Temporal
  stability came instead from per-cell history (#277: glyph reversals in a
  slow orbit fell from 100.6 to 0.1 per frame).
- **Definition comes from edges, not from the mask.** A pixel-exact mask
  silhouette cuts glyph cells in half, which reads as a cut-out with ASCII
  texture. The tavern became legible once geometry-only edges (depth,
  normals and mask id, never colour) drew line glyphs. These shipped as #267, on by default. They lie on the screen grid, so an object-grid variant gets none: one more reason the screen grid is the default.

### 2. Two stages, chosen per style

| Stage | Format | Resolution | Runs | For |
|---|---|---|---|---|
| `StyleStage::Hdr` | `Rgba16Float`, linear | scene (render scale) | after the scene pass, before SSAO/bloom/fog/tonemap | painterly filters that should be fogged, bloomed and graded (Kuwahara) |
| `StyleStage::Display` | surface format | output | after the composite, before FXAA and the HUD | crisp marks: glyphs, lines, hatching (ASCII, invert, mask view) |

The spike ran glyphs in HDR at scene resolution. Under `--render-scale 0.5`
that turned an 8 px cell into a blurred 16 px glyph, and bloom haloed every
bright glyph (findings 7-8). Display-stage styles see the upscaled,
tonemapped picture, so glyphs stay sharp at any render scale. Their cells
are sized in points, so they also look the same size on a high-DPI display.

### 3. A style is a descriptor and a pass

A style is one module in `crates/flint-render/src/stylize/` exporting
`STYLE: StyleDescriptor`, plus its name in `register_styles!`. The
descriptor carries the style's name, label, stage, numeric `params` with
ranges, string `options` with optional closed choices, optional UI `hints`,
`uses_mask`, an optional per-entity `component`, and `create`. `create`
returns a `StylePass` whose `encode` reads a `StyleFrame`.

Everything else is derived from the descriptor:

- **Scene format.** `[post_process.styles.<name>]`: numbers are params,
  strings are options, and unknown names or keys warn with the real ones.
- **F4 / F3 UI.** `flint_debug_ui::StylesEditor` draws every style from its
  descriptor (#268). A style needs no UI code to appear there.
- **CLI.** `flint render --style name:key=value`, `--sweep <style>.<key>=…`
  and `--look` all check keys against the descriptor (#271, #279).
- **Goldens.** Each style adds a case to
  `crates/flint-cli/tests/golden/stylize.goldens.toml` (#271).
- **Framework duties.** Lazy pipeline builds under `catch_unwind`; targets
  and bind groups live in `PostProcessResources` and are rebuilt on resize;
  the chain's single linear sampler (DX12's 2048-slot heap); a GPU-timer
  scope per style; ping-pong targets, so styles of one stage chain in
  registration order without knowing their neighbours.

### 4. The entity style mask is a redraw, holding ids only

Per-entity styles read an `R16Uint` id per pixel (#263). It is drawn after
the scene pass by redrawing each styled entity's opaque static, box and
skinned draws with their cached transform (and bone) bind groups. It is
depth-tested in the shader against the frame's single-sample depth, with a
slope tolerance. It is drawn only while a `uses_mask` style is on.

We did not add an extra colour attachment to every scene pipeline. The
redraw cost under 0.1 ms in the spike (about 0.02 ms for six entities at
1080p on an RTX 4070 as shipped). It needs nothing from the scene
pipelines, and it already works under MSAA and render scale.

Per-entity settings are **variants**: each distinct set of an entity's
style fields gets its own id (`ascii{ramp=blocks}`, `ascii@barrel`,
`ascii{off}`). The pass looks up each pixel's settings by its id. Entities
with the same settings share an id.

## Alternatives considered

- **Full screen only.** Simplest, and one grid by construction. Rejected
  because it can't leave the rest of a world smooth around a few glyph
  props, which was the point of the request. It survives as the
  "everything" case of the same pass.
- **Per entity on object grids by default.** Rejected: it caused visible
  seams wherever styled things met, and its stability gain was partial
  (above).
- **The mask as an attachment on every scene pipeline.** Rejected: it
  touches every pipeline and vertex format and makes the scene pass pay
  even with no style on. The redraw is opt-in and was measured as cheap.
- **Store anchor and parameters per pixel** (the spike's 16-byte
  `Rgba32Float` mask). Rejected for an id-only `R16Uint`, with each
  variant's settings in a table the pass indexes.
- **More branches in the composite shader** (the render-mode pattern).
  Rejected: every look would have to edit the core shader. Neither the UI
  nor the CLI could discover the look, and it could not chain with other
  looks.
- **HDR-only placement.** Rejected for glyph-like styles (blur under
  render scale, bloom haloes). It is kept for painterly ones.

## Consequences

- A new style is a module, a shader and a golden. Watercolor (#272) and
  pencil (#273) build on the framework rather than on the renderer.
  User-authored WGSL (#274/#275) has a descriptor shape to target.
- Mixed scenes stay coherent: smooth world, glyph props, one grid.
- The mask covers static glTF, procedural boxes and skinned meshes. It does
  not cover terrain, grass, particles, sprites, water or transparents
  (#290, #310). Those can be stylized only by the whole-frame case.
- A per-entity variant is drawn per pixel on its own grid and keeps no
  temporal history yet, so a styled prop can still shimmer in motion
  (#277 covers the whole frame only).
- Limits as shipped: one glyph set a frame (since #335: up to 8, stacked
  in one atlas, each variant its own), and 63 variants among
  mask ids below 256 (#336). A whole-frame ASCII pass costs 2-3x once any
  entity has an `ascii_style` (#337).
- Kuwahara keeps its legacy `kuwahara_*` fields beside the style table
  (#298). Styles run in registration order within a stage unless a
  table's `order` key places them (#299).
- The spike's code (`ascii_spike.rs`, `--ascii=full|entity|entity-object`)
  is gone (#293); `docs/design/ascii-spike.md` keeps its findings, and
  the per-entity goldens (`ascii-style-entities`, `-screen`, `-object`)
  pin the shipped style.

## Measured

At 1920x1080 on an RTX 4070 (`flint render --gpu-time 240`):

| | GPU ms |
|---|---|
| Full-screen ASCII (pbr_test, after #277's two-pass split) | ~0.04 |
| Per-entity ASCII pass + mask (`demo/ascii_entities`) | ~0.06 + 0.03 |
| Same scene, `--ascii-scope screen` | 0.3-0.45 |
| Entity style mask, six entities (MSAA 1 / 4) | 0.02 / 0.03 |

Phones and the web are not measured yet (#270).
