# Stylize Passes

A stylize pass is a full-screen effect that changes how the picture looks rather than how the light behaves: the painterly Kuwahara filter, ASCII, the pencil sketch, and the watercolor style planned after them (epic #260). They share one framework in `flint-render` (`crates/flint-render/src/stylize/`), so a new style is mostly a shader and a parameter list.

This page is the reference. For choosing between a whole-frame and a per-entity style, which grid glyphs sit on and which stage a style runs at, with the captures the choice was made from, see the guide [Stylized Rendering](../guides/stylized-rendering.md) and [ADR 0111](https://github.com/chrischaps/flint/blob/main/docs/adr/0111-stylized-rendering.md).

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
WARN unknown style 'watercolour' in [post_process.styles]; registered: kuwahara, ascii, pencil, invert, mask_view
WARN style 'invert' has no parameter 'wobble'; it takes: strength
```

Kuwahara predates the table and keeps its own `kuwahara_*` fields (see [Post-Processing](post-processing.md#kuwahara-painterly-filter)).

From the command line, `--style name[:key=value,...]` turns any registered style on over a scene without editing it (#271), on `flint render` and on `flint edit <model or effect> --render`:

```bash
flint render demo/pbr_test.scene.toml --style ascii
flint render demo/stylize_props.scene.toml --style ascii:ramp=detailed,color=tint,cell=10
flint render demo/pbr_test.scene.toml --style invert:strength=0.5 --style kuwahara:radius=6
flint render levels/tavern.scene.toml --style ascii:off       # the scene's table off (entities' ascii_style stay on, #376)
flint edit demo/models/barrel.glb --render barrel.png --style ascii:cell=6
flint render demo/pbr_test.scene.toml --style help            # every style and its keys
```

The keys are checked against the style's declaration: a number for a parameter, text for an option (one of its choices when it lists them), `off` or `enabled=false`. A mistake names what would have worked (`--style ascii has no key 'colour'; it takes: cell=8, ..., color=source|tint|raw, off`). `kuwahara:` takes `radius`, `sharpness`, `hardness` and `anisotropy`, its `kuwahara_*` fields. `--style-time <s>` fixes the post time animated styles read (headless captures default to 0).

In the player and the scene viewer, **F4 → Styles** lists every style that reads the table, each in a section of its own drawn from the style's declaration (#268). A style needs no UI code to appear there:

- **A row per setting.** A combo per option with a closed list of choices, a text field per option that takes any name (a font; it applies on Enter), a slider per parameter, and one colour picker for each `<base>_r`, `<base>_g`, `<base>_b` triple (ASCII's `tint`). The rows are labelled, explained on hover, and step by whole numbers where the style says so ([hints](#writing-a-style)).
- **↺** beside a value that isn't the default puts it back.
- **Defaults** clears the style's values. **Authored** puts back what the scene says, on and off included. **Copy TOML** puts the `[post_process.styles.<name>]` block that reproduces what's on screen on the clipboard, with only the values that differ from the defaults, so a look tuned by eye goes back into the scene in one paste:

  ```toml
  [post_process.styles.ascii]
  color = "palette"
  palette = "amber"
  cell = 16
  levels = 5
  glow = 0.77
  ```

- **Against the scene.** The header says *edited* while the style differs from the scene's own table, and a dot marks each value that does (hover it for the scene's value).

![F4's Styles section in the player: the meadow under an authored amber ASCII look, with cell size, tone bands and glow tuned by eye. Each tuned value has a dot because it now differs from the scene, and Copy TOML has just copied the block above](../images/f4-styles-editor.png)

The same editor is the player's **F3 Styles** panel, which appears with the other scene panels when the scene uses a style (a `[post_process.styles]` table or a `style_mask` entity). It also lists the scene's styled entities, the style each asks for, and the hue the [mask view](#the-entity-style-mask) paints that style in.

| Style | Stage | Parameters |
|-------|-------|------------|
| `kuwahara` | HDR | the `kuwahara_*` fields |
| `ascii` | Display | see [ASCII](#ascii) |
| `pencil` | Display | see [Pencil](#pencil) |
| `invert` | Display | `strength` 0--1 (default 1): how far toward the negative |
| `mask_view` | Display | `shading` 0--1 (0.8), `dim` 0--1 (0.35), `outline` 0--4 px (1): shows the [entity style mask](#the-entity-style-mask) |

`invert` is the template style: the smallest one that exercises the framework.

## ASCII

The full-screen ASCII style (#265) turns every cell of one screen grid into a glyph from a [glyph set](glyph-atlas.md). The cell's luminance picks a step of the ramp, so glyph density carries brightness, and the glyph is inked in the cell's colour or a tint.

![The ASCII style on pbr_test: the two boxes and the character in the classic ramp, 8 px cells](../images/ascii-style-pbr-test.png)

*`flint render demo/pbr_test.scene.toml --ascii --ascii-no-edges` at 1280x720, cropped at 1:1: the tone glyphs alone. With [edges](#edges), the default, the boxes and the character are outlined as well.*

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
| `edges` | `1` | Line glyphs along [edges](#edges), mixed over the tone glyph: 0 turns the edge passes off, 1 draws the line alone |
| `edge_threshold` | `0.5` | How much edge, in cell widths, a cell needs to draw a line glyph (a line straight across is 1) |
| `edge_max` | `3` | More edge than this is texture-like geometry (grass, foliage) and keeps its tone glyph. A styled entity's outline ignores it |
| `edge_depth` | `0.6` | The relative view-distance step that counts as a silhouette (lower finds more) |
| `edge_normal` | `2.5` | The normal change, as a Sobel over normals rebuilt from depth, that counts as a crease (lower finds more) |
| `temporal` | `on` | Temporal stability (#277), below. `off` draws every frame as if it were the first |
| `smoothing` | `0.6` | How much of last frame's colour a cell keeps (0--0.95). A cell whose colour jumps takes the new one at once, so nothing trails |
| `hysteresis` | `0.35` | How far, in ramp steps, a cell's tone may stray from its glyph's band before the glyph changes (0--1) |
| `dither` | `0` | Ordered (4x4 Bayer) dither before the ramp, fixed to the grid: gradients break into a still pattern instead of one hard edge (0--1, `--ascii-dither`) |

### Temporal stability

In motion, a cell whose tone sits on a ramp boundary tips to the neighbouring glyph and back at the slightest change, and the whole picture crawls. So the style draws in two passes: a cell pass writes each cell's colour and glyph into a small state texture (one texel a cell), and the glyph pass draws from it. The state is kept for the next frame. A cell blends in part of last frame's colour (`smoothing`) and keeps last frame's glyph until its tone has clearly moved on (`hysteresis`).

On `demo/ascii_motion.scene.toml`, a slow orbit of the meadow clearing captured at 1280x720 with `--frame-step 0.0166667` for 3 s (181 frames, 14,400 cells), `python tools/ascii_flips.py` counted:

| | glyph flips per frame | reversals per frame (A, B, A) |
|---|---|---|
| `temporal = "off"` | 408.3 (2.84% of cells) | 100.6 (0.70%) |
| `temporal = "on"` | 118.3 (0.82%) | 0.1 (0.00%) |

Reversals, where a glyph changes and changes back two frames later, are the shimmer itself, and they are gone. The flips that remain are the scene really moving.

The state counts as history only when the last frame the renderer drew was the one before, and the grid, ramp and size are unchanged. A single `flint render` therefore draws exactly what the stateless style drew, and a `--frame-step` capture gives the style the same frames every run. The player's captures can still differ from run to run by one level in a few pixels, with the history on or off (#341). The history absorbs those differences rather than carrying them forward. The player calls `SceneRenderer::reset_style_history()` when a new scene comes up. Call it on a camera cut too, so the new shot doesn't start from the old one's glyphs.

It is a display-stage style: it reads the finished, tonemapped picture at output resolution, after the render-scale upscale, so glyphs stay sharp at `--render-scale 0.5`. The grid is centred on the frame, so the part cells at the borders match. At a whole-number scale (8 px glyphs in 8, 16 or 24 px cells) glyphs are drawn exactly; at a fractional one, such as 12 px cells on a 1.5x display, each glyph pixel keeps one width and only its borders blend. It costs about 0.04 ms at 1080p on an RTX 4070 (`--gpu-time 240`), down from about 0.15 ms before #277 split it in two: the cell pass averages each cell's colour once, where the old single pass did it for every pixel. That is with `edges = 0`. The [edge passes](#edges) add about 0.4 ms (measured on the tavern before #277, as 0.15 against 0.53--0.67 ms).

| Key | Default | |
|-----|---------|---|
| `scope` | `screen` | `screen`: the table glyphizes the whole frame. `entities`: only the entities that ask (below), the table being their preset |
| `grid` | `screen` | `screen`: one grid centred on the frame. `object`: each styled entity's cells pinned to its projected origin (below) |

[Edges](#edges) (#267) draw line glyphs along silhouettes and creases, and [per-entity ASCII](#per-entity-ascii) (#266) applies the style to chosen entities. Its goldens are in the [stylize goldens](#goldens).

### Colour modes

The glyph's shape carries brightness; its colour sets the mood (#276). Every capture below is `demo/pbr_test.scene.toml` at 1280x720, cropped at 1:1 around the two boxes and the character. They were made before edges (#267) were on by default: add `--ascii-no-edges` to get them exactly, or leave it off and the boxes are outlined too.

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

### Edges

Luminance alone loses a silhouette wherever an object is about as bright as what's behind it. So cells along the scene's edges draw the glyph set's line glyphs, `─ │ ╱ ╲` in the built-in set, picked by the direction of the edge through the cell (#267). Away from edges cells keep their tone glyphs.

![A sphere, a box and a pillar in ASCII, each outlined in unbroken line glyphs](../images/ascii-edges-on.png)

![The same scene with --ascii-no-edges: the sphere's top half melts into the ground behind it](../images/ascii-edges-off.png)

*`demo/ascii_edges.scene.toml` at 1280x720, cropped at 1:1, with edges (top) and with `--ascii-no-edges`. The sphere and the box carry `style_mask { style = "ascii" }`; the pillar has none, so it is outlined by depth alone.*

Edges come from geometry, never from colour, so a wood texture makes none but a table's outline and its creases do (the ASCII spike's finding, #261). Three passes, before the glyph pass:

1. **Edge pixels**, at the depth's resolution. A 3x3 Sobel over the relative view distance finds silhouettes; one over normals rebuilt from the depth finds creases, counted only where the 5x5 patch is one surface. Where an ASCII-styled entity (an `ascii_style`, or a `style_mask { style = "ascii" }`, [the entity style mask](#the-entity-style-mask)) meets anything else, including another styled entity with different settings, the pixel is an **outline**. Silhouettes and outlines keep their near side only, so each is drawn once.
2. **Cells**, one fragment a glyph cell. It reads every edge pixel in its cell, not a sparse lattice, so a one-pixel outline can't slip between samples, and sums their structure tensors: the dominant orientation picks the glyph. The cell draws a line when it holds at least `edge_threshold` cell widths of edge and at most `edge_max` (an outline needs only the threshold).
3. **The glyph pass** draws the cell's line glyph. A cell holding under one cell width of line yields to a neighbour across the line holding more, so a line that runs along a cell border isn't drawn twice.

Under a render scale the edge pixels are the scene's, so a line is still one cell wide. The editor grid of `flint render` (on unless `--no-grid`) writes depth, so its lines are traced too; renders of the look pass `--no-grid`.

Edges come with the colour modes: line glyphs are inked as the cell's tone glyph would be, cut out of the ink under `ground = "inverse"`, and glow around them. The edge cells lie on the screen grid, so per-entity variants on that grid get them, and a variant on `grid = "object"` keeps its tone glyphs. Edge cells keep no history: under #277's temporal stability the tone glyphs hold still and the lines follow the geometry each frame. The [stylize goldens](#goldens) pin pbr_test without edges (the #265 golden, unchanged), with edges, and the sphere and box above.

## Per-entity ASCII

An entity with an `ascii_style` component is drawn in glyphs while the rest of the scene stays smooth (#266). No `[post_process.styles.ascii]` table is needed:

![demo/ascii_entities.scene.toml: the meadow and the ground stay smooth while the bench (amber blocks), the skinned YBot (its own colour), the barrel (on its own grid) and the crate with its lantern (phosphor green) are glyphs; the lantern's glass box stays smooth](../images/ascii-entities.png)

*`flint render demo/ascii_entities.scene.toml --no-grid --target 0.7,0.7,4.2 --distance 5.2 --yaw 29 --pitch 10`, 1280x720.*

```toml
[post_process.styles.ascii]   # optional: the preset every styled entity starts from
scope = "entities"            # without it, the table would glyphize the whole frame
background = 0.3

[entities.bench.ascii_style]  # only what differs from the preset
ramp = "blocks"
color = "tint"
tint = [1.0, 0.70, 0.30]      # a 3-array sets tint_r, tint_g, tint_b
levels = 4

[entities.ybot.ascii_style]   # empty: the preset as it is (skinned meshes work too)

[entities.lantern_glass.ascii_style]
enabled = false               # stays smooth, even under a whole-frame ASCII
```

- **Fields.** Every key of the table above except `font` and `scope`, all optional. A field the entity leaves out comes from the scene's table, then the style's default. The schema gives no field a default on purpose, since the loader would fill it in and hide the scene's setting. An archetype's `[archetype.<name>.defaults.ascii_style]` is a preset too, and an entity's own fields merge over it field by field.
- **Descendants** inherit their ancestor's style until one carries an `ascii_style` or `style_mask` of its own; the nearest wins. A plain `style_mask { style = "ascii" }` is the same as an empty `ascii_style`.
- **Ids.** Entities with the same settings share one style mask id. Each distinct set of settings is a variant, and the pass looks up each pixel's settings by its id. One frame holds 63 variants among mask ids below 256; more are drawn like the whole frame, with a warning.
- **Colour.** A cell's colour averages only the samples of the pixel's own variant, so a styled prop's glyphs carry its colour, not the meadow's behind it. Its silhouette is the mask's, to the pixel.
- **The whole frame.** With the scene's table on and `scope = "screen"` (or `--ascii`), everything is glyphs and entities still change their own settings; `enabled = false` keeps one smooth.
- **Grids.** By default every styled entity shares the screen's grid, which keeps glyphs aligned where props meet (the spike's finding, #261). `grid = "object"` pins an entity's cells to its projected origin: its glyphs hold still while it slides across the screen, but still swim when it turns or comes closer (#277). Each such entity gets a variant of its own.
- **Colour modes** (`color`, `palette`, `ground`, `glow`) are per entity too: a frame packs up to 32 palette inks in all, and a palette that doesn't fit beside the others uses the scene's, with a warning.
- **Edges between entities** come from [edges](#edges) (#267). Without them a styled prop's edge is only where its glyph field stops, so two styled props that overlap would run together. The edge passes draw line glyphs along depth steps and creases, and an outline wherever the style mask changes from one of the style's ids to any other: the plain `ascii` id or any variant's, so a change between two variants is an outline too. Ids are per variant, not per entity, so two props with the same settings that touch on one surface are told apart by the depth and crease edges, not the mask. Variants on `grid = "object"` don't draw edges yet, because the edge cells are on the screen grid.
- **Temporal stability** (#277's `smoothing`, `hysteresis` and `dither`) applies to the whole frame's cells. A per-entity variant is drawn per pixel on its own grid and keeps no history yet, so a styled prop can still shimmer in motion.
- **From scripts** (#269): `set_entity_style(id, "ascii")`, `set_style_param(id, "cell", 12.0)` and `clear_entity_style(id)` edit the component at runtime, and changing a param every frame makes no GPU allocation. See [Scripting → Style API](scripting.md#style-api) and `demo/ascii_story.scene.toml`.
- **Not yet:** a per-entity `font` (one glyph set a frame; a different one warns and draws in the scene's), and ramps beyond 128 steps in all across the frame's variants (a ramp that doesn't fit uses the scene's, with a warning).

```bash
flint render demo/ascii_entities.scene.toml --output ascii.png              # as authored
flint render demo/ascii_entities.scene.toml --ascii-scope screen            # the whole frame
flint render demo/pbr_test.scene.toml --ascii --ascii-grid object           # flags set the preset
```

At 1080p on an RTX 4070 the demo's ASCII pass costs about 0.045 ms and its mask 0.013--0.023 ms (DX12, Vulkan); with `--ascii-scope screen` the pass costs about 0.06 ms, and with no styled entities 0.04 ms. In Chrome it is about twice that, and a render scale leaves it unchanged, since the cells are output pixels. The numbers by backend, scale and browser are in `docs/design/web.md` ("Stylize passes", #270). It works under `--msaa 4` and `--render-scale`, where the silhouette follows the scaled mask. F4 → Styles → ASCII offers `scope` and `grid` beside the other settings, and `flint edit --watch` hot-reloads `ascii_style` edits.

## Pencil

The `pencil` style (#273) draws the scene in graphite on paper. The picture's tone becomes layers of cross-hatching. The scene's silhouettes and creases become contour lines that wobble and double the way quick sketch lines do. Both sit on a paper with grain, whose tooth breaks the strokes up.

```toml
[post_process.styles.pencil]
spacing = 6.0          # hatch line spacing, points
thickness = 1.0        # stroke width, points
layers = 3             # hatch layers, 0-4: each joins, crossing the last, as the tone darkens
direction = "surface"  # surface | screen | tensor
angle = 50.0           # the base angle, degrees
contrast = 1.2         # tone contrast before it picks layers (brightness too)
contour = 1.0          # outline strength; contour_width in points
depth = 1.0            # outline sensitivity to depth steps; crease to creases
detail = 0.35          # outlines from the picture's own luminance edges
wobble = 1.5           # how far lines wander, points
sketchy = 0.6          # a second, offset outline beside the first
grain = 0.5            # paper grain, and how much it breaks strokes
shade = 0.25           # soft rubbed graphite in the darks
paper = [0.95, 0.93, 0.88]   # display RGB; fills paper_r/g/b (lead_r/g/b likewise)
lead = [0.16, 0.16, 0.19]
color = 0.0            # 1 draws over the scene's own colour: coloured pencil
seed = 0               # another whole number is another hand
boil = 0.0             # redraws a second, for boiling lines (0 = still)
```

```bash
flint render demo/stylize_props.scene.toml --no-grid --style pencil -o page.png
flint render demo/pencil_sketch.scene.toml --no-grid -o props.png        # per-entity variants
flint render scene.toml --style pencil --sweep pencil.direction=surface,screen,tensor -o dirs.png
```

![The pencil sketch demo drawn as a whole page: the ground and sky in graphite, the props keeping their own pencil settings (coloured cask, blue YBot)](../images/pencil-sketch-page.png)

How it draws:

- **Hatching builds tone the way a hand does.** Each layer switches on as the tone darkens past its threshold, and its strokes thicken from nothing rather than popping in. A layer crosses the one before it off the square (at 0°, +65°, −40° and +25°), so layers never make a grid. Each stroke is a dash of its own length that tapers at both ends, is jittered off its row and is bent by a slow wobble. A few dashes are left out.
- **Stroke direction.** `surface` runs strokes along the surface's iso-depth lines, taken from the gradient of 1/z. That gradient is constant over a plane, so a box face hatches straight, and it turns smoothly round a barrel and across a floor. `screen` hatches everything at `angle`. `tensor` follows the picture's luminance edges (a blurred structure tensor), which gets busy around shadows. Where a surface or tensor direction is unsure (a face turned square to the camera, a flat colour), `angle` shows through. Directions are quantized to 15° bins with neighbouring bins blended, so strokes stay straight within a patch, as hand hatching does.
- **Contours** come from depth steps (drawn on the near side), creases (the normal rebuilt from depth turns), the edge of a styled entity's mask, and luminance edges by `detail`. They are read back through a noise offset (`wobble`), twice when `sketchy` is set.
- **Deterministic.** Every random choice is an integer hash of the stroke's row, dash, layer and `seed`, so the same seed draws byte-identical frames. `boil` moves the seed in whole steps of post time, which `--style-time` fixes.

![The three stroke directions on stylize_props](../images/pencil-directions.png)

![Close up: the crate's faces hatched straight, its outline doubled, the ground cross-hatched in three layers](../images/pencil-closeup.png)

It runs at the display stage, at output resolution, with sizes in points (times the pixels-per-point scale), in two passes. The first pass writes each pixel's tone, contour strength and stroke direction into a scratch target. The second blurs the direction into a flow and draws the strokes. At 1080p on an RTX 4070 the whole frame costs about 0.49 ms. With only two props asking for it the cost is 0.19 ms, because both full-screen passes still run and only return early on unstyled pixels.

**Per entity.** Pencil works per entity in the same way as ASCII. A `pencil_style` component, or a plain `style_mask { style = "pencil" }`, draws that entity in pencil while the rest of the frame stays as it is, and each distinct set of settings becomes a variant with its own mask id. Every field of the table above except `scope` can be set on the component, and a field it leaves out comes from the scene's table, then the default. `grid = "object"` pins an entity's hatching to its projected origin, so its strokes travel with it. With `scope = "entities"` the scene's table is only a preset. A pixel that another style owns (an ASCII prop in a pencil frame) is left to that style.

```toml
[post_process.styles.pencil]
scope = "entities"     # a preset for the entities below
spacing = 5.0

[entities.cask.pencil_style]
color = 0.6            # coloured pencil over the cask's own colour
lead = [0.34, 0.17, 0.07]

[entities.ybot.pencil_style]
lead = [0.10, 0.18, 0.45]
sketchy = 1.0

[entities.tag.pencil_style]
enabled = false        # stays painted
```

![demo/pencil_sketch.scene.toml: six props, each its own hand; the world around them stays painted](../images/pencil-sketch-entities.png)

### Blending in and out

`blend` draws an entity part-way into its style, from 0 (its own shading) to 1 (the style's, and the default when it's left out). A script ramps it to fade a prop into text and back (#278):

```toml
[entities.barrel.ascii_style]
blend = 0.5
transition = "dissolve"   # or "fade", the default
```

```rust
fn on_init() {
    let barrel = get_entity("barrel");
    set_entity_style(barrel, "ascii");
    set_style_param(barrel, "blend", 0.0);          // styled, but nothing shows yet
    set_style_param(barrel, "transition", "dissolve");
}
fn on_update() {
    let k = (total_time() - 1.0) / 3.0;             // 0 -> 1 from 1 s to 4 s
    let k = if k < 0.0 { 0.0 } else if k > 1.0 { 1.0 } else { k };
    set_style_param(get_entity("barrel"), "blend", k);
}
```

![The barrel dissolving into green phosphor glyphs at blend 0, 0.25, 0.5, 0.75 and 1: its cells turn whole, in ragged patches that spread](../images/ascii-blend-dissolve.png)

![The bench cross-fading into amber blocks at blend 0, 0.25, 0.5, 0.75 and 1: every pixel a mix of its wood and its glyph](../images/ascii-blend-fade.png)

*Frames from `flint-player demo/ascii_reveal.scene.toml --frame-step 0.05 --screenshot-every 0.05`, cropped and doubled.*

- **`fade`** mixes every pixel of the entity with its glyph pixel by `blend`, in display space.
- **`dissolve`** turns whole cells: a cell is a glyph once `blend` passes its place in a dissolve order, half patch noise a few cells across and half per-cell hash, so glyphs arrive in ragged patches that spread. The order is fixed per cell, so a ramp never shimmers, and on `grid = "object"` it travels with the prop. On average the share of turned cells equals `blend`. A small prop holds few cells, so its share varies more: for a 6x5-cell prop at blend 0.5, the middle 80% of placings turn 30--73% of its cells; for 24x18 cells, 42--58%.
- **It rides in the mask, not the variant.** The [style mask](#the-entity-style-mask) carries each pixel's blend in its `.g` channel beside the id in `.r`. Two props fading at different rates still share one id, and a ramp makes no new variant, row or GPU object (a GPU test in `tests/stylize.rs` ramps it and counts). Like the id, the blend is the styled root's, inherited by its subtree.
- **`style_mask { blend = ... }`** works too, for a style with no component of its own. A style component's `blend` wins over the `style_mask`'s on the same entity.
- **Under a whole-frame style**, a prop at blend 0 is a smooth hole in the glyph field: the blend says how much of the entity's *own* variant shows, and below 1 the rest is the picture as it came.
- The `style` debug view draws each entity's hue as strong as its blend.
- **Cost.** The blend costs one more channel in the mask (`Rg16Uint`, 4 bytes a pixel) and no extra texture read. Pixels a blend leaves plain skip the glyph work: at 1080p `demo/stylize_blend.scene.toml`'s ASCII pass took 0.31 ms, against 0.39 ms for the same props fully styled.

`demo/ascii_reveal.scene.toml` is a 10-second loop: the barrel dissolves into phosphor glyphs, the bench fades into amber blocks, and both go back. The `ascii-blend` golden pins a half-way dissolve and a 40% fade.

### Style blends and render-mode tears

`--render-mode` 1--4 (matrix, blood, drunk, tron) and a style blend can both look like reality tearing into text, but they work at different levels:

| | Render-mode tears | Style blend |
|---|---|---|
| Where | Composite pass, before display styles | Display-stage style, after the composite |
| What it covers | The whole screen, through `tear_mask`: drifting fbm patches or an iris from the centre, in screen space | One entity and its subtree, through the style mask, on the entity's own cells |
| Driven by | `mode_mix` and `mode_params` (a scene-wide value) | `blend` on each entity's component |
| The look | Fixed per mode (mode 1's glyphs are an 8x12 dot-matrix built into `composite_shader.wgsl`) | Any style's settings: fonts, ramps, palettes, edges, grids |

Neither duplicates the other's job. Tears change how the *world* is seen, and a blend changes how one *thing* is drawn. They compose: an ASCII prop inside a matrix tear is glyphized from the tear's picture, since display styles read the composite's output. Mode 1's glyphs and the ASCII style do overlap as looks, and the `matrix` look (`--look matrix`) is the ASCII style's take on it. Letting a style's blend follow the tear mask, so props turn to text where reality tears, is left for later.

## Looks and sweeps

A **look** (#279) is a named preset you can put over any scene. It lives in `styles/<name>.look.toml`, next to the `*.style.toml` generation guides, and has two tables. `[look]` says what the look is and which sweeps to try first. `[post_process]` is a block in exactly the scene format. The look's block is merged into the scene's own `[post_process]` key by key: tables merge and everything else replaces, so a look changes only the keys it names. A field or style the engine doesn't know is an error that names the closest real one, because otherwise a typo would quietly fall back to a default.

```toml
[look]
name = "graphite"
description = "Pencil sketch: cross-hatched tone, wobbling contours, paper grain"
tune = ["pencil.spacing=4,6,9", "pencil.contrast=1,1.4,1.8"]
notes = "Spacing is the pencil's sharpness..."

[post_process]
bloom_enabled = false

[post_process.styles.pencil]
spacing = 5.0
contrast = 1.4
wobble = 1.8
```

Four looks ship, compiled into the CLI so they work from a game repo as well. A `styles/<name>.look.toml` beside the scene, or in its parent folder, takes the place of a built-in with the same name.

| Look | Built from |
|------|------------|
| `ascii_noir` | mono ASCII, black ground, hard contrast, heavy vignette |
| `matrix` | phosphor-palette ASCII with glow, `detailed` ramp |
| `ink_wash` | Kuwahara radius 6, desaturation, warm paper fog and grade, grain (stands in until watercolor, #272) |
| `graphite` | the pencil style at a 5-point spacing, contrast 1.4, doubled outlines (inverted mono ASCII before #273) |

```bash
flint render --look help                                   # every look, its tune sweeps and notes
flint render scene.toml --look ink_wash -o wash.png
flint render scene.toml --look ink_wash --style kuwahara:radius=3 -o sharper.png   # flags apply over the look
```

**`--sweep KEY=V1,V2,...`** renders the same frame once per value and lays the frames out as one labelled contact sheet, four across, in the ASCII style's built-in glyphs. KEY can be:

- any `--style` key, written `<style>.<key>`: `ascii.cell=6,8,12`, `ascii.ramp=classic,blocks`, `kuwahara.radius=3,6,9`. Values are checked against the style's range;
- any `[post_process]` field, written `post.<field>`: `post.exposure=0.8,1.2`. Use `|` between values to sweep a vector: `post.fog_color=0,0,0|0.7,0.75,0.82`;
- `look=ascii_noir,matrix,ink_wash,graphite`, to compare whole looks.

Each cell is 640x360 unless you pass `--width` and `--height`. The scene loads once and every column re-renders the same frame, so four columns of `stylize_props` take about 13 s in a debug build. A sweep goes over `--look` and every other flag.

The ASCII looks set no `edges` key, so they draw the [edge glyphs](#edges) by default; add `edges = 0` to a look's `[post_process.styles.ascii]` for tone alone. The two sweeps below were captured before edges landed, so the props in them aren't outlined.

![The four looks on stylize_props, one sweep](../images/look-sweep-stylize-props.png)

![ascii_noir's brightness swept: at 0 the ground is black, at 0.15 it comes back, at 0.45 the props drown](../images/look-sweep-noir-brightness.png)

### Choosing cell size and ramp

- **The cell is in points, not image pixels.** A 640-wide sweep cell at `cell = 8` is 80 glyphs across, while the same look at 1920 is 240. Use a sweep to compare values against each other, then confirm the winner with one render at the final size. Glow and fine ramps in particular read differently at full size.
- **Size by columns.** About 200–240 columns (cell 8–9 at 1080p) keeps props readable. Below about 120 columns (cell 16 and up at 1080p) the image becomes a poster: silhouettes only, no surface detail. Cells of 4–5 read as texture or hatching, not as letters.
- **Pick a ramp for the tones the scene has.** `classic` (` .:-=+*#%@`, 10 steps) is the general-purpose ramp. `detailed` (70 glyphs sorted by coverage) gives the smoothest gradients and the most letter-like texture, so it suits small cells. `minimal` (` .oO@`) and `blocks` (` ░▒▓█`) have five steps each: they band hard, which suits big cells and posters.
- **Brightness first, then contrast.** When a scene comes out mostly black or mostly blank, sweep `brightness` before `contrast`. On `stylize_props` with `ascii_noir` at brightness 0, contrast 1.6 and 2.2 left the ground black. Contrast 0.6 or 1 brought the ground back, but it also flattened the props into it. Brightness 0.15 at contrast 1.6 brought the ground back and kept the props dark against it.

## Where styles run

Each style belongs to one of two stages:

```text
scene (HDR, render scale)
  -> style mask (Rg16Uint: id + blend, only while a style reads it)
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
    hints: &[StyleHint::new("strength", "Strength", "0 = none, 1 = the full negative")],
    component: None,               // Some("ascii_style"): entities take the style per entity (#266)
    create: |d| Box::new(Invert::new(d)),   // build pipelines in d.format
};
```

`hints` are optional, and F4 draws a style without them: a setting with no hint shows its own name (`tint_r` becomes "Tint r") on a plain slider. A hint gives a setting (or a colour triple, by its base name) a label, a hover line, and `.integer()` for a value that only makes sense whole (ASCII's cell size). Name a colour's channels `<base>_r`, `<base>_g` and `<base>_b`, each 0--1 in display values, and F4 folds them into one colour picker.

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
- **The command line.** `flint render --style <name>` reaches a new style from its descriptor alone.

## Goldens

Every registered style is pinned by a golden image in `crates/flint-cli/tests/golden/`, listed in `stylize.goldens.toml` (#271). `flint render-suite` renders each case with `flint render` and fails when one moves past its tolerance; `cargo test -p flint-cli --test render_goldens` runs the same suite.

```bash
flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml            # gate
flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml --case ascii-entity
flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml --bless    # after a deliberate change
```

| Case | What it pins |
|------|--------------|
| `ascii-full` | `--style ascii:edges=0` on `pbr_test`: the #265 golden, unchanged with edges off |
| `ascii-full-detailed-tint` | another ramp and the tint colour mode on `stylize_props`, with edges |
| `ascii-full-hidpi` | `--pixels-per-point 2`: cells in points (edges off: the editor grid would be traced) |
| `ascii-full-msaa4` | `--msaa 4` scene passes under the glyph grid (edges off) |
| `ascii-edges-pbr` | [edges](#edges) on `pbr_test`, grid off |
| `ascii-edges`, `ascii-edges-msaa4`, `ascii-edges-scale05` | the styled sphere and box of `demo/ascii_edges.scene.toml` outlined, at 1x, under MSAA 4 and at `--render-scale 0.5` |
| `ascii-entity`, `ascii-entity-object` | the spike's per-entity modes (#261), screen and object grids |
| `style-mask` | the entity style mask the per-entity styles read (`--debug-mode style`) |
| `invert`, `kuwahara` | the other styles, with a tighter tolerance |

Cases render single-sample with the post clock at 0 unless they say otherwise. The props cases use `demo/stylize_props.scene.toml`, which loads in a few seconds in a debug build, with the grid off. Glyph styles share #265's tolerance (fail when more than 2% of pixels differ by more than 48 levels, or the mean passes 3), since a cell on a ramp step's border can flip glyph on another GPU; smooth styles take `level = 8, share = 0.003, mean = 0.25`, because Kuwahara at radius 4 against a radius-5 golden is a 0.39 mean the glyph limits would pass. On one machine, every case is byte-identical run to run. **A new style adds a case here.** The manifest format is in [Headless Rendering](../guides/headless-rendering.md#golden-suites).

## The entity style mask

A style that should change only some entities (per-entity ASCII, watercolor or pencil) needs to know, per pixel, which entity drew it. The entity style mask is that answer: an `Rg16Uint` texture at the scene's resolution, with one id a pixel in `.r` (0 where no style asked) and the entity's [blend](#blending-in-and-out) in `.g` (65535 for 1, #278).

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

To read it in a style, set `uses_mask: true`, make a layout with `kit::mask_layout` and bind it with `frame.mask_bind(...)`; in WGSL it is a `texture_2d<u32>` read with `textureLoad`, the id in `.r` and the blend in `.g` (`stylize/mask_view.rs` and `mask_view.wgsl` show the whole thing). A per-entity style honours a blend by mixing its pixel with its input by `f32(m.g) / 65535.0`; one that ignores `.g` draws every entity at blend 1.

### Per-entity settings

A style that takes settings per entity names its component in its descriptor: `component: Some("ascii_style")`, with a schema in `schemas/components/`. The framework then does the rest:

- an entity carrying the component (or a `style_mask` naming the style) switches the style on for the frame, and the mask is drawn, with no `[post_process.styles]` table;
- the component's fields are read as the descriptor's params and options (a 3-array `x` fills `x_r`, `x_g`, `x_b`), and each distinct set is a `StyleVariant` with its own mask id, named for its settings (`ascii{cell=12,ramp=blocks}`, `ascii@barrel`, `ascii{off}` for `enabled = false`);
- a style whose `grid` option is `object` gets one variant per styled root, with `anchor` set to the root's world origin each frame (`frame.project(anchor)` puts it on screen).

In `encode`, `frame.variants()` lists this style's `(id, variant)` pairs, `frame.variant_param(v, "cell")` and `frame.variant_option(v, "ramp")` read a variant's settings with the scene's as fallback, and `frame.whole_frame()` says whether the scene's table covers everything. `stylize/ascii.rs` and `stylize/pencil.rs` pack them into a table indexed by mask id; watercolor (#272) can do the same.
