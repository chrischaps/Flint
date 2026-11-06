# Stylize Passes

A stylize pass is a full-screen effect that changes how the picture looks rather than how the light behaves: the painterly Kuwahara filter, and the ASCII, watercolor and pencil styles planned after it (epic #260). They share one framework in `flint-render` (`crates/flint-render/src/stylize/`), so a new style is mostly a shader and a parameter list.

## Using a style

A scene switches a style on with a table under `[post_process]`:

```toml
[post_process]
bloom_enabled = true

[post_process.styles.invert]
strength = 1.0          # a parameter: a number
```

A table switches its style on unless it says `enabled = false`. Every other key is one of the style's parameters (a number) or one of its options (a string, such as a font or a ramp). Unknown style names and keys are reported as warnings when the scene loads, with the names that do exist:

```text
WARN unknown style 'watercolour' in [post_process.styles]; registered: kuwahara, ascii, invert, mask_view
WARN style 'invert' has no parameter 'wobble'; it takes: strength
```

Kuwahara predates the table and keeps its own `kuwahara_*` fields (see [Post-Processing](post-processing.md#kuwahara-painterly-filter)).

In the player and the scene viewer, **F4 → Styles** lists every style that reads the table, drawn from the style's own declaration: a toggle, a slider per parameter, and a combo (or a text field) per option.

| Style | Stage | Parameters |
|-------|-------|------------|
| `kuwahara` | HDR | the `kuwahara_*` fields |
| `ascii` | Display | see [ASCII](#ascii) |
| `invert` | Display | `strength` 0--1 (default 1): how far toward the negative |
| `mask_view` | Display | `shading` 0--1 (0.8), `dim` 0--1 (0.35), `outline` 0--4 px (1): shows the [entity style mask](#the-entity-style-mask) |

`invert` is the template style: the smallest one that exercises the framework.

## ASCII

The full-screen ASCII style (#265) turns every cell of one screen grid into a glyph from a [glyph set](glyph-atlas.md). The cell's luminance picks a step of the ramp, so glyph density carries brightness, and the glyph is inked in the cell's colour or a tint.

![The ASCII style on pbr_test: the two boxes and the character in the classic ramp, 8 px cells](../images/ascii-style-pbr-test.png)

*`flint render demo/pbr_test.scene.toml --ascii` at 1280x720, cropped at 1:1.*

```toml
[post_process.styles.ascii]
cell = 8                # cell width in points; the height follows the font's aspect
font = "builtin"        # or a fonts/<name>.glyphs.toml set beside the scene
ramp = "classic"        # a ramp of that set; empty = the set's default_ramp
color = "source"        # source | tint | raw | mono | palette
palette = "phosphor"    # with color = "palette": a built-in or palettes/<name>.palette.toml
ground = "dim"          # dim | black | inverse
glow = 0.0              # halo around bright glyphs
```

```bash
flint render demo/pbr_test.scene.toml --ascii --schemas schemas --width 1280 --height 720
flint render demo/mesh_meadow.scene.toml --ascii --ascii-ramp blocks --ascii-tint 1,0.7,0.25
```

| Key | Default | |
|-----|---------|---|
| `cell` | `8` | Cell width in points (4--32). Times the window's scale factor, so cells look the same size on a high-DPI display; rounded to whole pixels |
| `font` | `builtin` | The glyph set: `builtin`, a set name (`fonts/<name>.glyphs.toml` in the scene's folder or its parent) or a `.glyphs.toml` path. One that fails to load falls back to the built-in set, with a warning |
| `ramp` | the set's default | A ramp of the set. The built-in set has `classic`, `blocks`, `minimal` and `detailed`. An unknown name warns once and uses the default |
| `color` | `source` | `source`: the cell's hue at full value. `tint`: one colour (`tint_r`, `tint_g`, `tint_b`, default phosphor green). `raw`: the cell's colour as it is, dark cells dark. `mono`: white glyphs on grey. `palette`: the inks of `palette` (see [Colour modes](#colour-modes)) |
| `palette` | `phosphor` | With `color = "palette"`: `cga`, `cga-warm`, `cga16`, `phosphor`, `amber`, `mono`, or a palette file. One that can't be found warns once and uses `phosphor` |
| `ground` | `dim` | Behind the glyph: `dim` (the mode's own: the cell's colour times `background`, or the palette's background), `black`, or `inverse` (the ink fills the cell and the glyph is cut out of it; the ramp runs backwards so bright cells stay bright) |
| `glow` | `0` | A halo around each glyph, stronger on bright cells (0--2; 0.6 reads as a phosphor tube). It's drawn in the pass, since a display-stage style runs after bloom |
| `background` | `0.15` | How much of the cell's colour fills behind its glyph: 0 is black, 1 the cell's colour |
| `brightness`, `contrast` | `0`, `1` | Luminance remap before the ramp: contrast about mid-grey, then an offset |
| `ink` | `1` | Glyph brightness |
| `levels` | `0` | Quantize the tone to this many bands (0 keeps every step of the ramp) |
| `footprint` | `1` | Average each cell's colour over this many cells (wider settles texture noise) |

It is a display-stage style: it reads the finished, tonemapped picture at output resolution, after the render-scale upscale, so glyphs stay sharp at `--render-scale 0.5`. The grid is centred on the frame, so the part cells at the borders match. At a whole-number scale (8 px glyphs in 8, 16 or 24 px cells) glyphs are drawn exactly; at a fractional one, such as 12 px cells on a 1.5x display, each glyph pixel keeps one width and only its borders blend. It costs about 0.15 ms at 1080p on an RTX 4070.

Edge glyphs (#267) and per-entity ASCII (#266) build on it. `crates/flint-cli/tests/ascii_golden.rs` holds its golden image.

### Colour modes

The glyph's shape carries brightness; its colour sets the mood (#276). Every capture below is `demo/pbr_test.scene.toml` at 1280x720, cropped at 1:1 around the two boxes and the character.

| | |
|---|---|
| ![source](../images/ascii-color-source-pbr-test.png) `--ascii` (source) | ![tint](../images/ascii-color-tint-pbr-test.png) `--ascii-color tint` |
| ![raw](../images/ascii-color-raw-pbr-test.png) `--ascii-color raw` | ![mono](../images/ascii-color-mono-pbr-test.png) `--ascii-color mono` |
| ![phosphor](../images/ascii-color-phosphor-pbr-test.png) `--ascii-palette phosphor --ascii-glow 0.6` | ![amber](../images/ascii-color-amber-pbr-test.png) `--ascii-palette amber --ascii-ramp blocks` |
| ![cga](../images/ascii-color-cga-pbr-test.png) `--ascii-palette cga` | ![cga16](../images/ascii-color-cga16-pbr-test.png) `--ascii-palette cga16` |
| ![inverse](../images/ascii-color-inverse-pbr-test.png) `--ascii-palette cga-warm --ascii-ground inverse` | ![black](../images/ascii-color-black-pbr-test.png) `--ascii-ground black` |
| ![tidepool](../images/ascii-color-tidepool-pbr-test.png) `--ascii-palette tidepool` (`demo/palettes/tidepool.palette.toml`) | |

And on `demo/mesh_meadow.scene.toml` (640x360 crops):

![The meadow in phosphor green](../images/ascii-color-phosphor-mesh-meadow.png)
![The meadow in amber blocks](../images/ascii-color-amber-mesh-meadow.png)
![The meadow in CGA warm, inverse](../images/ascii-color-inverse-mesh-meadow.png)

**Palettes** live in `flint_render::palette`, so any style can use them. A palette is a list of inks and a background, in display (sRGB) hex:

```toml
# palettes/tidepool.palette.toml, beside the scene or in its parent folder
match = "nearest"          # nearest (default) | luminance
background = "#0d1b2a"     # default black
colors = ["#1b998b", "#3a86ff", "#ed217c", "#ff9b71", "#fffd82", "#e8f1f2"]
```

`nearest` gives each cell the ink nearest its hue at full value (plain RGB distance), so a dark red still reads red and glyph density alone says it's dark; desaturated cells land on the palette's lightest neutral. `luminance` lists the inks darkest first and picks one by the cell's tone, as a one-colour phosphor tube does. Up to 32 inks. A file wins over a built-in of the same name; `PaletteCache` resolves names the way `GlyphSetCache` resolves fonts, through the vfs.

| Built-in | Match | Inks |
|---|---|---|
| `cga` | nearest | cyan, magenta, white on black (mode 4, palette 1, high intensity) |
| `cga-warm` | nearest | green, red, yellow on black (palette 0, high intensity) |
| `cga16` | nearest | the 15 text-mode colours on black |
| `phosphor` | luminance | four greens on near-black |
| `amber` | luminance | four ambers on near-black |
| `mono` | luminance | four greys on black |

On an RTX 4070 at 1080p the pass costs 0.143 ms in `source`, 0.152 ms matching `cga16`'s 15 inks, 0.144 ms with `ground = "inverse"` and 0.220 ms with `glow` on (24 more glyph reads a pixel).

## Where styles run

Each style belongs to one of two stages:

```text
scene (HDR, render scale)
  -> style mask (R16Uint ids, only while a style reads it)
  -> HDR styles (Kuwahara)          linear Rgba16Float, scene resolution
  -> SSAO, volumetric, bloom
  -> composite: fog, DoF, tonemap, grade, vignette, grain, render modes
  -> Display styles (ASCII, invert) surface format, output resolution
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
    options: &[],                  // named strings: StyleOption { name, default, choices }
    enabled: None,                 // None: read `enabled` from the style table
    uses_mask: false,              // true: the renderer draws the entity style mask for it
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
| `option(name)` | the style's option (a string) from the config, or its default |
| `pixels_per_point` | the window's scale factor (1 headless, or `flint render --pixels-per-point`), for styles sized in points |
| `scene_dir` | the scene's folder when the host knows it, for styles that load files (ASCII's glyph sets) |
| `depth`, `camera` | the scene's depth (sample by UV) and the inverse view-projection, position, near and far |
| `mask`, `mask_bind(...)`, `style_id(name)` | the [entity style mask](#the-entity-style-mask), while a `uses_mask` style is on |
| `target(name, format)` | a texture of the style's own at the stage's size, kept until the next resize |
| `sampled(...)`, `sampled_input(...)`, `uniform(...)`, `bind(...)`, `depth_bind(...)` | bind groups kept with the post chain |
| `first_write(name)` | true once per chain, for uniforms that only change on resize |
| `time` | game seconds, for animated styles |

What the framework does so a style doesn't have to:

- **Lazy building.** A style's pipelines are built the first frame it is on, under `catch_unwind` (some drivers crash compiling shaders); one that fails stays off. Hosts can call `SceneRenderer::ensure_style_resources` after setting a config to move the compile off that frame.
- **Resize.** Every target and cached bind group lives in the post chain's resources, which `resize_postprocess()` rebuilds. There is no per-style resize hook to forget.
- **Formats.** HDR-stage styles draw in `Rgba16Float`; display-stage styles in the surface format. On an sRGB surface a display style samples linear values and its writes are encoded, so a style that works in display values decodes and encodes itself (`frame.srgb()`; `invert.wgsl` shows how).
- **Samplers.** Styles bind the chain's one linear sampler, so their bind groups add nothing to DX12's 2048-slot sampler heap.
- **The mask.** A `uses_mask` style gets the entity style mask drawn for it each frame, and no style pays for it otherwise.
- **Timing.** Each style is a GPU-timer scope under its `label`, so `flint render --gpu-time` and the player's `--perf` name it.

## The entity style mask

A style that should change only some entities (per-entity ASCII, watercolor or pencil) needs to know, per pixel, which entity drew it. The entity style mask is that answer: one `R16Uint` id a pixel, at the scene's resolution, 0 where no style asked.

An entity asks with the `style_mask` component:

```toml
[entities.barrel.style_mask]
style = "pencil"        # entities sharing a name share an id

[entities.lantern_glass.style_mask]
enabled = false         # out of any style, including an inherited one
```

Descendants inherit their ancestor's style until one of them carries a `style_mask` of its own. Ids are numbered from 1 in name order, so the same scene gets the same ids on every run; a style finds its own with `frame.style_id("pencil")`.

![The style mask debug view over demo/style_mask.scene.toml: the bench and the skinned YBot in one hue ("ascii"), the barrel, the crate and the lantern it carries in another ("pencil"), the opted-out box on the crate grey](../images/style-mask-debug.png)

See it with the `style` debug view, which switches the `mask_view` style on:

```bash
flint render demo/style_mask.scene.toml --debug-mode style --output mask.png
```

Each style is a hue over the scene's shading, unstyled pixels are grey, and a white line marks where two ids meet. In the player and viewer it is **Style Mask** in F4's debug shading list.

How it's drawn:

- **A redraw, not a new attachment.** After the scene pass, each visible opaque draw of a styled entity is drawn again into the mask with its cached transform bind group (and bone bind group for skinned meshes). The scene pipelines don't change and the per-entity loop makes no buffers or bind groups; the ids go up in one write a frame.
- **Depth without a depth buffer.** A mask fragment tests itself against the scene's single-sample depth with a tolerance of twice its own depth slope, so grass or a wall in front of a styled prop cuts its mask. Under MSAA it tests the sample-0 resolve, so a feature a pixel thin can lose the odd pixel; under `--render-scale` the mask is at the scaled size, and a display-stage style reads it by UV.
- **Only when wanted.** The mask target and its pipelines exist only once a style with `uses_mask` (or the debug view) is on. At 1080p on an RTX 4070 the pass costs about 0.02 ms (0.03 ms under MSAA 4) for the demo scene's six styled entities.
- **What's masked.** Static glTF meshes, procedural boxes and skinned meshes. Particles, sprites, grass, terrain and water never are.

To read it in a style, set `uses_mask: true`, make a layout with `kit::mask_layout` and bind it with `frame.mask_bind(...)`; in WGSL it is a `texture_2d<u32>` read with `textureLoad` (`stylize/mask_view.rs` and `mask_view.wgsl` show the whole thing).
