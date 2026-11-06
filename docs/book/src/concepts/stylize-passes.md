# Stylize Passes

A stylize pass is a full-screen effect that changes how the picture looks rather than how the light behaves: the painterly Kuwahara filter, and the ASCII, watercolor and pencil styles planned after it (epic #260). They share one framework in `flint-render` (`crates/flint-render/src/stylize/`), so a new style is mostly a shader and a parameter list.

## Using a style

A scene switches a style on with a table under `[post_process]`:

```toml
[post_process]
bloom_enabled = true

[post_process.styles.invert]
strength = 1.0          # every key other than `enabled` is a number
```

A table switches its style on unless it says `enabled = false`. Unknown style names and parameters are reported as warnings when the scene loads, with the names that do exist:

```text
WARN unknown style 'watercolour' in [post_process.styles]; registered: kuwahara, invert
WARN style 'invert' has no parameter 'wobble'; it takes: strength
```

Kuwahara predates the table and keeps its own `kuwahara_*` fields (see [Post-Processing](post-processing.md#kuwahara-painterly-filter)).

In the player and the scene viewer, **F4 → Styles** lists every style that reads the table, with a toggle and a slider per parameter, drawn from the style's own declaration.

| Style | Stage | Parameters |
|-------|-------|------------|
| `kuwahara` | HDR | the `kuwahara_*` fields |
| `invert` | Display | `strength` 0--1 (default 1): how far toward the negative |

`invert` is the template style: the smallest one that exercises the framework.

## Where styles run

Each style belongs to one of two stages:

```text
scene (HDR, render scale)
  -> HDR styles (Kuwahara)          linear Rgba16Float, scene resolution
  -> SSAO, volumetric, bloom
  -> composite: fog, DoF, tonemap, grade, vignette, grain, render modes
  -> Display styles (invert)        surface format, output resolution
  -> FXAA -> surface -> HUD
```

- **HDR stage.** Linear light at the scene's render scale. Everything after it shades the style: fog fades it, bloom haloes its bright strokes, the grade tints it. SSAO and bloom read the unstylized scene.
- **Display stage.** The finished, tonemapped picture at output resolution, after the render-scale upscale and before FXAA and the HUD. This is where glyphs and lines stay crisp under `--render-scale` (the ASCII spike's finding).

Styles of one stage run in registration order, each reading the one before it. The framework ping-pongs between two targets it owns, so any number of styles chain without a style knowing its neighbours.

## Writing a style

A style is one module in `crates/flint-render/src/stylize/` plus its name in the `register_styles!` list at the bottom of `stylize/mod.rs`. The module exports a `StyleDescriptor` called `STYLE`:

```rust
pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "invert",                // the [post_process.styles.<name>] key
    label: "Invert",               // F4 and --gpu-time
    stage: StyleStage::Display,
    params: &[StyleParam { name: "strength", default: 1.0, min: 0.0, max: 1.0 }],
    enabled: None,                 // None: read `enabled` from the style table
    create: |d| Box::new(Invert::new(d)),   // build pipelines in d.format
};
```

and the `StylePass` that `create` returns records the style's draws for one frame:

```rust
impl StylePass for Invert {
    fn encode(&self, frame: &StyleFrame<'_>, encoder: &mut wgpu::CommandEncoder,
              input: &StyleInput<'_>, output: &wgpu::TextureView) {
        // write uniforms from frame.param("strength"), then draw one
        // full-screen triangle that samples `input` into `output`
    }
}
```

`stylize::kit` has the parts most styles are made of: `fullscreen_pipeline`, `uniform_layout`, `sampled_layout`, `uniform_buffer`, `shader` and `fullscreen_pass`. `stylize/invert.rs` and `invert.wgsl` together are about 140 lines.

What a style gets from `StyleFrame`:

| | |
|---|---|
| `width`, `height`, `format`, `texel_size()`, `srgb()` | the stage's size and format |
| `param(name)` | the style's parameter from the config, or its default |
| `depth`, `camera` | the scene's depth (sample by UV) and the inverse view-projection, position, near and far |
| `mask` | a per-pixel style id, once the entity style mask lands (#263); `None` today |
| `target(name, format)` | a texture of the style's own at the stage's size, kept until the next resize |
| `sampled(...)`, `sampled_input(...)`, `uniform(...)`, `bind(...)`, `depth_bind(...)` | bind groups kept with the post chain |
| `first_write(name)` | true once per chain, for uniforms that only change on resize |
| `time` | game seconds, for animated styles |

What the framework does so a style doesn't have to:

- **Lazy building.** A style's pipelines are built the first frame it is on, under `catch_unwind` (some drivers crash compiling shaders); one that fails stays off. Hosts can call `SceneRenderer::ensure_style_resources` after setting a config to move the compile off that frame.
- **Resize.** Every target and cached bind group lives in the post chain's resources, which `resize_postprocess()` rebuilds. There is no per-style resize hook to forget.
- **Formats.** HDR-stage styles draw in `Rgba16Float`; display-stage styles in the surface format. On an sRGB surface a display style samples linear values and its writes are encoded, so a style that works in display values decodes and encodes itself (`frame.srgb()`; `invert.wgsl` shows how).
- **Samplers.** Styles bind the chain's one linear sampler, so their bind groups add nothing to DX12's 2048-slot sampler heap.
- **Timing.** Each style is a GPU-timer scope under its `label`, so `flint render --gpu-time` and the player's `--perf` name it.
