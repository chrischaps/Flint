# Stylize Passes

A stylize pass is a full-screen effect that changes how the picture looks rather than how the light behaves: the painterly Kuwahara filter, ASCII, the pencil sketch and watercolor (epic #260). They share one framework in `flint-render` (`crates/flint-render/src/stylize/`), so a new style is mostly a shader and a parameter list.

This page is the reference. For choosing between a whole-frame and a per-entity style, which grid glyphs sit on and which stage a style runs at, with the captures the choice was made from, see the guide [Stylized Rendering](../guides/stylized-rendering.md) and [ADR 0111](https://github.com/chrischaps/flint/blob/main/docs/adr/0111-stylized-rendering.md).

## Using a style

A scene switches a style on with a table under `[post_process]`:

```toml
[post_process]
bloom_enabled = true

[post_process.styles.invert]
strength = 1.0          # a parameter: a number
```

A table switches its style on unless it says `enabled = false`. Every other key is one of the style's parameters (a number) or one of its options (a string, such as a font or a ramp). Unknown style names and keys are reported as warnings when the scene loads, with the names that do exist, and so is a value outside an option's closed list of choices (#318: a colour mode, a ground, a scope; a ramp or a palette only suggests its list, since a glyph set brings its own ramps and a palette may be a file). The value is kept as written and draws as the default. A number outside its parameter's range (the range `--list-styles` and F4 give, a colour's channels included) warns too, and is kept as written (#355): a style may clamp it or draw it badly, so it rarely draws as asked. `flint validate` reports the same warnings:

```text
WARN unknown style 'watercolour' in [post_process.styles]; registered: kuwahara, watercolor, ascii, pencil, invert, mask_view
WARN style 'invert' has no parameter 'wobble'; it takes: strength
WARN styles.ascii.color: unknown value 'plaid'; it takes: source, tint, raw, mono, palette (drawing as 'source')
WARN styles.invert.strength=3 is outside its range 0..1; kept as written, past what the style is tuned for
```

Kuwahara's table is `[post_process.styles.kuwahara]`, and the contour's `[post_process.styles.contour]`. Scenes from before the tables spell them `kuwahara_enabled`, `kuwahara_radius`, `contour_width` and so on in `[post_process]`; those keys are folded into the tables when the scene loads (see [Post-Processing](post-processing.md#kuwahara-painterly-filter)).

From the command line, `--style name[:key=value,...]` turns any registered style on over a scene without editing it (#271), on `flint render` and on `flint edit <model or effect> --render`:

```bash
flint render examples/pbr_test.scene.toml --style ascii
flint render examples/stylize_props.scene.toml --style ascii:ramp=detailed,color=tint,cell=10
flint render examples/pbr_test.scene.toml --style invert:strength=0.5 --style kuwahara:radius=6
flint render examples/stylize_props.scene.toml --style ascii:color=tint,tint=#ffb340   # a colour: #rrggbb or r/g/b
flint render levels/tavern.scene.toml --style ascii:off       # off everywhere: the table and every entity's ascii_style (#376)
flint edit examples/models/barrel.glb --render barrel.png --style ascii:cell=6
flint render examples/pbr_test.scene.toml --style help            # every style and its keys
```

A word is checked before anything draws, and a mistake stops the command with what would have worked: an unknown style or key names the ones that exist, and a number outside its parameter's range names the range (#355), as `--sweep` does for each column:

```text
error: invalid value 'ascii:cell=400' for '--style <STYLES>': ascii: cell=400 is outside its range 4..32
```

The windowed hosts take the same words (#328): `flint play`, `flint-player` and the scene viewer (`flint edit scene.toml`), with `--style-time` beside them. They write the words over the scene's `[post_process]` when it loads and again after each scene transition (the player), each hot reload and each authored/default swap in F4 (the viewer), so the command line keeps winning over the file. F4 and the F3 Styles panel still edit the live result, and their Authored button goes back to what the scene says. On the web the query takes them as `?style=ascii:cell=12`, repeated for more than one (`?style=ascii&style=invert`).

```bash
flint play examples/stylize_props.scene.toml --style ascii
flint-player examples/stylize_props.scene.toml --style pencil:spacing=5 --style-time 1.5
flint edit examples/stylize_props.scene.toml --watch --style ascii:cell=10
```

### Listing the styles

`flint render --list-styles` prints every registered style (#303): its stage, where a scene sets it, its options with their choices and its parameters with their defaults, F4 ranges and whether they step by whole numbers, each with its hover help. Add `--json` for an agent; give a scene and the user styles in its `styles/` folder are listed too:

```bash
flint render --list-styles
flint render examples/user_styles.scene.toml --list-styles --json
```

```text
kuwahara (Kuwahara) [hdr, builtin]
  [post_process.styles.kuwahara]
  radius      4  [1 .. 8, integer]  — The brush: each sector's reach in scene pixels; bigger is more painterly and costs more
  sharpness   8  [1 .. 16]  — How strongly the least varied sector wins; lower blends sectors more softly
  ...
invert (Invert) [display, builtin]
  [post_process.styles.invert]
  strength  1  [0 .. 1]  — How far toward the negative: 0 = none, 1 = full
```

Each JSON entry has `name`, `label`, `stage` (`hdr` or `display`), `source` (`builtin` or `user`), `table`, `component` (the per-entity component, or null), `uses_mask`, `params` (`name`, `key` as the scene writes it, `default`, `min`, `max`, `integer`, `color` (the colour it is a channel of, or null), `label`, `help`), `options` (`name`, `default`, `choices`, `open`, `label`, `help`; empty choices take any text, and `open` marks choices found beside the scene, which any other name a style can resolve, a file path, may join) and `colors` (`key`, `default` as `[r, g, b]`, `channels`, `label`, `help`; #315). The text form lists a colour once, by its key. A range is the F4 menu's: `--style` refuses a value outside it, and a scene's table warns and keeps it (#355). Every table also takes `enabled`, `order` and `blend` ([Blending a whole-frame style](#blending-a-whole-frame-style)). The list is read from the same declarations as F4 and `--style`, so a new style appears in it with nothing added (`flint_render::stylize::catalog`).

The keys are checked against the style's declaration: a number for a parameter, text for an option (one of its choices when the list is closed), a [colour](#colours) as `#rrggbb` or `r/g/b` (`tint=0.3/1/0.4`: commas split the keys, so not `r,g,b`), `off` or `enabled=false`. A mistake names what would have worked (`--style ascii has no key 'colour'; it takes: cell=8, ..., color=source|tint|raw, off`). `--style-time <s>` fixes the post time animated styles read (headless captures default to 0).

In the player and the scene viewer, **F4 → Styles** lists every style that reads the table, each in a section of its own drawn from the style's declaration (#268). A style needs no UI code to appear there:

- **A row per setting.** A combo per option with a list of choices, a text field per option that takes any name (it applies on Enter), a slider per parameter, and one colour picker for each `<base>_r`, `<base>_g`, `<base>_b` triple (ASCII's `tint`). The rows are labelled, explained on hover, and step by whole numbers where the style says so ([hints](#writing-a-style)). A setting the current options don't read is greyed out, and its hover line says why: in ASCII's `source` colour mode, Tint and Palette are both idle until `color` is `tint` or `palette`.
- **The scene's own files** (#322). An option can find its choices at run time (`StyleOption::discovered`), and ASCII's do: `palette` offers the built-ins, then every `palettes/*.palette.toml` the scene can see; `font` offers `builtin`, then every `fonts/*.glyphs.toml` it can see; and `ramp` offers the ramps the chosen font has, so a project's set lists its own names rather than the built-in four. "Can see" is [the one asset search](assets.md#where-the-engine-looks-for-a-file): the folder beside the scene, in each directory above it up to the project root, then in the mounted engine, a nearer file hiding a farther one of its name. The list is the resolver's own (`Project::list`), so every name offered is one the style will find, and a scene two folders down is offered the project's palettes. The folders are read through the vfs, so a bundle on the web lists the same files. The list is found when the scene loads (and reloads) and when another option it depends on changes, not every frame. A value the scene names that isn't listed (a palette by its path) still shows, selected. `flint render <scene> --list-styles` lists the same choices.
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

- **Commit to scene** (#364) writes that same block into the scene file, as the Grass, Ocean and Camera panels' Commit to File do for theirs. It works key by key through `toml_edit` (`SceneDocument::set_style_table`). A changed value is rewritten in place and keeps its comments. A key back at its default is removed, and a comment on its own line above that key goes with it. A key the style doesn't know is left alone. Comments and key order elsewhere in the file survive. A style that is off with nothing else set loses its table, and on Kuwahara and contour the old `kuwahara_*` / `contour_*` keys in `[post_process]` are removed. After the write the editor's authored settings are the committed ones, so the *edited* marker and the dots clear. The viewer reloads the file as it does any scene edit. Because that reload would drop entity edits not yet saved, the button is greyed out until Ctrl+S. A scene played from a bundle (`flint-player --bundle`, the web) has no file to write, so the button isn't drawn there.
- **Against the scene.** The header says *edited* while the style differs from the scene's own table, and a dot marks each value that does (hover it for the scene's value).

![F4's Styles section in the player: the meadow under an authored amber ASCII look, with cell size, tone bands and glow tuned by eye. Each tuned value has a dot because it now differs from the scene, and Copy TOML has just copied the block above](../images/f4-styles-editor.png)

The same editor is the player's **F3 Styles** panel, which appears with the other scene panels when the scene uses a style (a `[post_process.styles]` table or a `style_mask` entity). It also lists the scene's styled entities, the style each asks for, and the hue the [mask view](#the-entity-style-mask) paints that style in.

| Style | Stage | Parameters |
|-------|-------|------------|
| `kuwahara` | HDR | `radius` (whole pixels), `sharpness`, `hardness`, `anisotropy`, `boil`; see [Post-Processing](post-processing.md#kuwahara-painterly-filter) |
| `watercolor` | Display | see [Watercolor](#watercolor) |
| `ascii` | Display | see [ASCII](#ascii) |
| `pencil` | Display | see [Pencil](#pencil) |
| `palette` | Display | `palette`, `dither` 0--1 (0), `pixel` 1--32 points (1); see [Palette](#palette) |
| `invert` | Display | `strength` 0--1 (default 1): how far toward the negative |
| `mask_view` | Display | `shading` 0--1 (0.8), `dim` 0--1 (0.35), `outline` 0--4 px (1): shows the [entity style mask](#the-entity-style-mask) |
| `contour` | HDR | `width`, `threshold`, `strength`, `luma`, `color`, `fade`, `boil`: ink lines, last of the HDR chain; see [Post-Processing](post-processing.md#contour-ink-lines) |

`invert` is the template style: the smallest one that exercises the framework.

## ASCII

The full-screen ASCII style (#265) turns every cell of one screen grid into a glyph from a [glyph set](glyph-atlas.md). The cell's luminance picks a step of the ramp, so glyph density carries brightness, and the glyph is inked in the cell's colour or a tint.

![examples/ascii_lantern.scene.toml: two crates under a lamp post, a barrel and a figure on a black ground, against a thin band of dusk under a starred sky, in the classic ramp at 8 px cells](../images/ascii-lantern.png)

*`flint render examples/ascii_lantern.scene.toml --width 1280 --height 720`, with no other flags: the scene's `[camera]` frames it and its `[post_process.styles.ascii]` block sets the look.*

The style rewards value contrast. Glyph density follows luminance, so a scene lit for PBR, mid-greys everywhere, turns into an even field of `=` and `-`. `examples/ascii_lantern.scene.toml` (#316) is authored for glyphs instead: a near-black ground that stays empty, crates lit warm from one side by a lamp so each has a bright face and a dark one, and a figure and a barrel standing against the last band of a low sun under a dark, starred sky. Its block raises `contrast` to 1.5, which pushes the dim sky to black and keeps the glyphs for what is lit. The ground's top sits 2 cm above y = 0, so the editor grid `flint render` draws by default is hidden under it instead of lifting the empty ground into dashes. The same scene in each built-in ramp:

![The lantern scene in the classic, blocks, minimal and detailed ramps](../images/ascii-lantern-ramps.png)

*`flint render examples/ascii_lantern.scene.toml --sweep ascii.ramp=classic,blocks,minimal,detailed`*

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
flint render examples/ascii_lantern.scene.toml --width 1280 --height 720   # authored for the style
flint render examples/pbr_test.scene.toml --ascii --width 1280 --height 720
flint render examples/mesh_meadow.scene.toml --ascii --ascii-ramp blocks --ascii-tint 1,0.7,0.25
```

| Key | Default | |
|-----|---------|---|
| `cell` | `8` | Cell width in points (4--32). Times the window's scale factor, so cells look the same size on a high-DPI display; rounded to whole pixels |
| `font` | `builtin` | The glyph set: `builtin`, a set name (`fonts/<name>.glyphs.toml`, found by [the one asset search](assets.md#where-the-engine-looks-for-a-file)) or a `.glyphs.toml` path from the scene's folder or one above it. One that fails to load falls back to the built-in set, with a warning that lists every path tried |
| `ramp` | the set's default | A ramp of the set. The built-in set has `classic`, `blocks`, `minimal`, `detailed`, `quadrants` and `braille`. An unknown name warns once and uses the default |
| `color` | `source` | `source`: the cell's hue at full value. `tint`: one colour (`tint`, default phosphor green). `raw`: the cell's colour as it is, dark cells dark. `mono`: white glyphs on grey. `palette`: the inks of `palette` (see [Colour modes](#colour-modes)) |
| `tint` | `[0.35, 1.0, 0.45]` | With `color = "tint"`: the ink, a [colour](#colours) (`[r, g, b]` 0--1 in display values, or `"#59ff73"`). `--ascii-tint r,g,b` sets it |
| `palette` | `phosphor` | With `color = "palette"`: `cga`, `cga-warm`, `cga16`, `phosphor`, `amber`, `mono`, `paper` (with `ground = "inverse"`), or a palette file. One that can't be found warns once and uses `phosphor` |
| `chroma` | `1` | With a `nearest` palette: scale each cell's saturation (OKLab a/b) before its ink is matched, so a muted scene reaches the palette's colours (0--4; 0 matches on lightness alone). See [Palettes](#palettes-match-in-oklab) |
| `palette_dither` | `0` | With a `nearest` palette: mix each cell's two nearest inks by a 4x4 Bayer pattern on the cell grid, in proportion to how far the cell sits between them (0--1), so a gradient steps through inks instead of banding into one |
| `ground` | `dim` | Behind the glyph: `dim` (the mode's own: the cell's colour times `background`, or the palette's background), `black`, or `inverse` (the ink fills the cell and the glyph is cut out of it; the ramp runs backwards so bright cells stay bright) |
| `glow` | `0` | A halo around each glyph, stronger on bright cells (0--2; 0.6 reads as a phosphor tube). It's drawn in the pass, since a display-stage style runs after bloom, so it stops at the cell's edge: for glow that crosses cells, add [display bloom](#display-bloom) |
| `background` | `0.15` | How much of the cell's colour fills behind its glyph: 0 is black, 1 the cell's colour |
| `brightness`, `contrast` | `0`, `1` | Luminance remap before the ramp: contrast about mid-grey, then an offset |
| `exposure` | `0` | Stops of gain on each cell's colour, in linear light, before its glyph and ink are picked (-4--4, `--ascii-exposure`). See [dark props](#dark-props-exposure-and-normalize) |
| `normalize` | `0` | Stretch the style's own luminance range over the ramp, measured from the same frame: the whole frame's, or each styled entity's (0--1, `--ascii-normalize`; #289) |
| `ink` | `1` | Glyph brightness |
| `levels` | `0` | Quantize the tone to this many bands (0 keeps every step of the ramp) |
| `footprint` | `1` | Average each cell's colour over this many cells (wider settles texture noise) |
| `edges` | `1` | Line glyphs along [edges](#edges), mixed over the tone glyph: 0 turns the edge passes off, 1 draws the line alone |
| `edge_threshold` | `0.5` | How much edge, in cell widths, a cell needs to draw a line glyph (a line straight across is 1) |
| `edge_max` | `3` | More edge than this is texture-like geometry (grass, foliage) and keeps its tone glyph. A styled entity's outline ignores it |
| `edge_depth` | `0.6` | The relative view-distance step that counts as a silhouette (lower finds more) |
| `edge_normal` | `2.5` | The normal change, as a Sobel over normals rebuilt from depth, that counts as a crease (lower finds more) |
| `edge_corner` | `0.9` | A cell whose edge has no one direction (its tensor's coherence under this) draws the [corner or tee](#corners) its neighbours' lines meet in, as do line ends and gaps (#637); 0 turns every corner off (#348) |
| `edge_ink` | `cell` | What inks the line glyphs ([Edge ink](#edge-ink), #351): `cell` (as the cell's tone glyph), `color` (`edge_color`) or `hue` (the near side's own colour) |
| `edge_color` | `[0.85, 0.82, 0.74]` | The line glyphs' ink for `edge_ink = "color"`, a warm off-white; `[r, g, b]` or `"#rrggbb"` |
| `edge_tint` | `0` | For `edge_ink = "color"`: mix the colour toward the near side's hue (0--1) |
| `outline` | `1` | Per-entity only: ink a styled entity's own pixels this many points from its silhouette (0--4, at least one pixel when on; 0 = off), so features thinner than a cell keep their shape ([Silhouettes](#silhouettes), #288) |
| `min_coverage` | `0.5` | Per-entity only: the least ink of the pixels in a cell the entity only grazes (0--1) |
| `silhouette` | `"pixel"` | Per-entity only: `pixel` cuts the glyphs at the entity's exact edge; `cell` draws every edge cell whole, as the entity's glyph or as the scene, by how much of it the entity covers ([Whole-cell silhouettes](#whole-cell-silhouettes), #338) |
| `cell_share` | `0.5` | Per-entity only, with `silhouette = "cell"`: the share of a cell's 16 colour samples the entity must cover for the cell to be its (1/16--1) |
| `temporal` | `on` | Temporal stability (#277), below. `off` draws every frame as if it were the first. `reproject` reads each cell's past where its surface was last frame ([Reprojected history](#reprojected-history), #344) |
| `smoothing` | `0.6` | How much of last frame's colour a cell keeps, per 60 Hz frame (0--0.95): at 120 Hz each frame keeps its square root, at 30 Hz its square, so a change settles in the same game time at any frame rate (#342). A cell whose colour jumps takes the new one at once, so nothing trails |
| `hysteresis` | `0.35` | How far, in ramp steps, a cell's tone may stray from its glyph's band before the glyph changes (0--1) |
| `dither` | `0` | Ordered (4x4 Bayer) dither before the ramp, fixed to the grid: gradients break into a still pattern instead of one hard edge (0--1, `--ascii-dither`) |

### Temporal stability

In motion, a cell whose tone sits on a ramp boundary tips to the neighbouring glyph and back at the slightest change, and the whole picture crawls. So the style draws in two passes: a cell pass writes each cell's colour and glyph into a small state texture (one texel a cell), and the glyph pass draws from it. The state is kept for the next frame. A cell blends in part of last frame's colour (`smoothing`, scaled by the frame's length in game time: the player's clock, so the fixed step under `--frame-step`) and keeps last frame's glyph until its tone has clearly moved on (`hysteresis`).

On `examples/ascii_motion.scene.toml`, a slow orbit of the meadow clearing captured at 1280x720 with `--frame-step 0.0166667` for 3 s (181 frames, 14,400 cells), `python tools/ascii_flips.py` counted:

| | glyph flips per frame | reversals per frame (A, B, A) |
|---|---|---|
| `temporal = "off"` | 408.3 (2.84% of cells) | 100.6 (0.70%) |
| `temporal = "on"` | 118.3 (0.82%) | 0.1 (0.00%) |

Reversals, where a glyph changes and changes back two frames later, are the shimmer itself, and they are gone. The flips that remain are the scene really moving.

`flint render --frames 181 --orbit 6.8755 --flip-stats` measures the same thing headlessly and exactly, from the style's own grid ([Frame sequences](../guides/headless-rendering.md#frame-sequences), #345). With `edges = 0` it gives 105.1 flips and 0.0 reversals a frame with the history on, against 321.1 and 57.8 with it off. The edge glyphs (#267), which came after this measurement, don't go through the history yet. With them on, the history leaves 30.3 reversals a frame (#477).

The player's capture of the same orbit (`tools/ascii_flips.py`, edges on, at b3448e5) agrees: 3.53% flips and 0.82% reversals a frame with `off`, 1.90% and 0.25% (35.5 a frame) with `on` (#481, the same finding as #477).

The state counts as history only when the last frame the renderer drew was the one before, and the grid, ramp and size are unchanged. A single `flint render` therefore draws exactly what the stateless style drew, and a `--frame-step` capture gives the style the same frames every run. Until #341 the player's captures still differed by one level in a few pixels, history on or off: the grass compute's blades came out in a different order each run. Five captures of the motion bench are now byte-identical. The player calls `SceneRenderer::reset_style_history()` when a new scene comes up, and on a [camera cut](#camera-cuts).

#### Camera cuts

After a cut (a cutscene's new shot, a teleport, a respawn) the history belongs to the old shot. Kept, it blends the old shot's colours into the new one's first frames, and cells whose tone barely changed keep the old glyph through the hysteresis band, for as long as the shot lasts. Two things reset it (#343):

- **Scripts** call `reset_style_history()` on the frame of the cut ([Scripting → Style API](scripting.md#style-api)). The next frame is drawn as if it were the first.
- **The player watches the camera** and resets by itself when a frame's step is both large and sudden: a move of more than 2 m or a turn of more than 30° in one frame, at more than four times the rate of the frame before. The second test keeps a fast chase camera, or a hitch that stretches one frame, from resetting every frame and losing its stability. A missed cut costs a shot's worth of stale glyphs, a false one costs one frame drawn without history. FOV and orthographic zoom changes aren't watched: a script that cuts by zoom calls `reset_style_history()`. `set_camera_cut_detection(false)` turns the watch off; the thresholds are the constants in `crates/flint-player/src/player_app/camera_cut.rs`.

`examples/ascii_cuts.scene.toml` cuts between two shots of the meadow clearing every second. Its script's `mode` is `"script"` (it resets at each cut), `"auto"` (the player's detection does) or `"none"` (neither). Captured at 1280x720 with `--frame-step 0.0166667` for 2.5 s, cells whose glyph differs from the `"script"` run (of 14,400):

| Mode | Before the first cut | First frame of the new shot | Its last frame, a second later | All frames after the cut |
|------|---|---|---|---|
| `"script"` | 0 | 0 | 0 | 0 |
| `"auto"` | 0 | 0 | 0 | 0 |
| `"none"` | 0 | 1,006 (7.0%) | 493 (3.4%) | 45,800 |

The carry-over is mostly in mid tones: `-` where the new shot has `=`, `.` and `:` in the grass. Half of it is still there when the shot ends, because cells that clear the hysteresis band have no reason to change.

#### Reprojected history

The history is kept in screen space: a cell's past is whatever was in that cell last frame. When the camera moves, that is another piece of the world. `temporal = "reproject"` (#344) follows the surface instead. The cell pass unprojects each cell's centre through the scene depth, projects it with last frame's camera, and reads the past from the cell that point fell in. Each cell also stores its centre's view depth. A past whose depth is more than 10% off where the surface should have been is something the surface was behind (a disocclusion), and the cell starts fresh. Only the camera's motion is followed: Flint writes no motion vectors, so a moving object's cells read the past of whatever was there, and the depth test turns most of that away.

It is correct: `reprojected_history_reads_the_past_of_the_surface_now_in_the_cell` (in `crates/flint-render/tests/stylize.rs`) trucks the camera one panel along a wall of red and green panels of equal luminance. With screen-space history, all 64 cells blend towards the other colour; reprojected, 16 do (the column that came on screen and the cells a panel edge crosses).

On the meadow it doesn't help, so it is an option and `on` stays the default. Set `[entities.camera_rig.orbit]` in `examples/ascii_motion.scene.toml` to `speed = 1.2` and `pan = true`, and the camera turns in place, sliding the picture about two cells a frame. These are 3 s captures at 1280x720 (181 frames, 14,400 cells), counted with `tools/ascii_flips.py`. `--track 8` counts flips along the picture's motion: for each 8x8 block of cells it finds the shift that best matches last frame. Without it, a glyph that travels with its surface would count as a flip in two cells. *Travel* is that tracker's mean shift in cells per frame, which shows whether glyphs move with the world:

| fast pan (1.2 rad/s) | flips | reversals | tracked flips | tracked reversals | travel |
|---|---|---|---|---|---|
| `off` | 16.34% | 4.20% | 10.23% | 1.67% | 1.10 |
| `on` | 7.19% | 1.09% | 6.28% | 0.91% | 0.30 |
| `reproject` | 15.75% | 3.56% | 8.80% | 1.61% | 1.19 |

| slow orbit (0.12 rad/s) | flips | reversals |
|---|---|---|
| `off` | 3.53% | 0.82% |
| `on` | 1.90% | 0.25% |
| `reproject` | 2.12% | 0.29% |

Reprojected glyphs do travel with the world (1.19 cells a frame, the same as the stateless style's 1.10). But flips along the motion fall only from 10.2% to 8.8%. The grid can't move, so a surface that slides 2.3 cells lands at a new place inside its cells. Each cell then averages a different patch of it, and the grass and ground really do change from one patch to the next. Screen-space history wins on every count because it does something else: it holds glyphs to the screen (0.30 cells a frame of travel) while the colours slide underneath. That doesn't read as lag. Each frame's cell colours are closer to the stateless frame's with `on` (3.01 levels in 255) than with `reproject` (3.54).

Reach for `reproject` when a scene has large flat surfaces whose colours differ in hue but not brightness. Smoothing lets go only on a jump in luminance, so screen-space history smears one such surface's colour into the next under a pan, as in the test above. It costs about 0.05 ms more than `on` at 1080p (0.46 against 0.41 ms for the whole style, `--gpu-time 240`).

It is a display-stage style: it reads the finished, tonemapped picture at output resolution, after the render-scale upscale, so glyphs stay sharp at `--render-scale 0.5`. The grid is centred on the frame, so the part cells at the borders match. At a whole-number scale (8 px glyphs in 8, 16 or 24 px cells) glyphs are drawn exactly; at a fractional one, such as 12 px cells on a 1.5x display, each glyph pixel keeps one width and only its borders blend. It costs about 0.04 ms at 1080p on an RTX 4070 (`--gpu-time 240`), down from about 0.15 ms before #277 split it in two: the cell pass averages each cell's colour once, where the old single pass did it for every pixel. That is with `edges = 0`. The [edge passes](#edges) add about 0.4 ms (measured on the tavern before #277, as 0.15 against 0.53--0.67 ms).

| Key | Default | |
|-----|---------|---|
| `scope` | `screen` | `screen`: the table glyphizes the whole frame. `entities`: only the entities that ask (below), the table being their preset |
| `grid` | `screen` | `screen`: one grid centred on the frame. `object`: each styled entity's cells pinned to its projected origin (below) |

[Edges](#edges) (#267) draw line glyphs along silhouettes and creases, and [per-entity ASCII](#per-entity-ascii) (#266) applies the style to chosen entities. Its goldens are in the [stylize goldens](#goldens).

### Colour modes

The glyph's shape carries brightness; its colour sets the mood (#276). Every capture below is `examples/pbr_test.scene.toml` at 1280x720, cropped at 1:1 around the two boxes and the character. They were made before edges (#267) were on by default: add `--ascii-no-edges` to get them exactly, or leave it off and the boxes are outlined too.

| | |
|---|---|
| ![source](../images/ascii-color-source-pbr-test.png) `--ascii` (source) | ![tint](../images/ascii-color-tint-pbr-test.png) `--ascii-color tint` |
| ![raw](../images/ascii-color-raw-pbr-test.png) `--ascii-color raw` | ![mono](../images/ascii-color-mono-pbr-test.png) `--ascii-color mono` |
| ![phosphor](../images/ascii-color-phosphor-pbr-test.png) `--ascii-palette phosphor --ascii-glow 0.6` | ![amber](../images/ascii-color-amber-pbr-test.png) `--ascii-palette amber --ascii-ramp blocks` |
| ![cga](../images/ascii-palette-cga-pbr-test-oklab.png) `--ascii-palette cga` | ![cga16](../images/ascii-palette-cga16-pbr-test-oklab.png) `--ascii-palette cga16` |
| ![inverse](../images/ascii-color-inverse-pbr-test.png) `--ascii-palette cga-warm --ascii-ground inverse` | ![black](../images/ascii-color-black-pbr-test.png) `--ascii-ground black` |
| ![tidepool](../images/ascii-color-tidepool-pbr-test.png) `--ascii-palette tidepool` (`examples/palettes/tidepool.palette.toml`) | |

And on `examples/mesh_meadow.scene.toml` (640x360 crops):

![The meadow in phosphor green](../images/ascii-color-phosphor-mesh-meadow.png)
![The meadow in amber blocks](../images/ascii-color-amber-mesh-meadow.png)
![The meadow in CGA warm, inverse](../images/ascii-color-inverse-mesh-meadow.png)

**Palettes** live in `flint_render::palette`, so any style can use them; the [palette style](#palette) is the other one that does. A palette is a list of inks and a background, in display (sRGB) hex:

```toml
# palettes/tidepool.palette.toml, beside the scene or any folder above it in the project
match = "nearest"          # nearest (default) | luminance
background = "#0d1b2a"     # default black
colors = ["#1b998b", "#3a86ff", "#ed217c", "#ff9b71", "#fffd82", "#e8f1f2"]
```

`nearest` gives each cell the ink nearest its hue at full value, so a dark red still reads red and glyph density alone says it's dark. Distance is measured in OKLab (#320, below), and the style's `chroma` and `palette_dither` shape the match. `luminance` lists the inks darkest first and picks one by the cell's tone, as a one-colour phosphor tube does. Up to 32 inks. A file wins over a built-in of the same name; `PaletteCache` resolves names the way `GlyphSetCache` resolves fonts, through the vfs, and hot-reloads the same way (#323): change an ink with the viewer or player open and the frame recolours within a second (half a second, measured on `examples/ascii_tidepool.scene.toml`), and a file that appears beside the scene takes over a built-in of its name. Natively only; a web bundle never changes.

| Built-in | Match | Inks |
|---|---|---|
| `cga` | nearest | cyan, magenta, white on black (mode 4, palette 1, high intensity) |
| `cga-warm` | nearest | green, red, yellow on black (palette 0, high intensity) |
| `cga16` | nearest | the 15 text-mode colours on black |
| `phosphor` | luminance | four greens on near-black |
| `amber` | luminance | four ambers on near-black |
| `mono` | luminance | four greys on black |
| `paper` | luminance | four inks, near-black to pale grey, on cream paper (#324). In ASCII, pair it with `ground = "inverse"` |

`paper` is the one palette whose ground is lighter than its inks, and ASCII's glyph density carries brightness: under the default `ground = "dim"` a dark field is a sparse scatter of dark glyphs on cream, so the meadow's grass reads as pale as its sky (top). With `ground = "inverse"` the cell is filled with its ink and the glyph is cut out of it in paper, so dark grass is dark and the sky stays light (bottom):

![The meadow in ASCII paper with the dim ground: grass and sky both pale](../images/ascii-paper-dim-mesh-meadow.png)
![The meadow in ASCII paper with the inverse ground: dark grass under a light sky](../images/ascii-paper-inverse-mesh-meadow.png)

*`--ascii --ascii-palette paper --ascii-ground dim|inverse` on `examples/mesh_meadow.scene.toml` at 1280x720, cropped at 1:1.*

#### Palettes match in OKLab

A `nearest` palette compares colours in [OKLab](https://bottosson.github.io/posts/oklab/) (#320), where the distance between two colours follows how different they look. Plain RGB distance read wrong in two ways. Under `cga16`, a full-value red `(1, 0, 0)` is nearer `#aa0000` than `#ff5555` in RGB, so saturated objects took the dim text-mode inks on top of the darkening that glyph density already does. Under `cga`, every muted colour went to white. OKLab fixes the first. Two style keys deal with the second, since a near-grey really is nearest the white ink:

- `chroma` scales the cell's OKLab a/b before matching. pbr_test's floor is a muted blue (`#636380`): at `chroma = 3` it is far enough from white to reach the cyan and magenta.
- `palette_dither` mixes the two nearest inks on a 4x4 Bayer pattern fixed to the cell grid. A cell takes the second ink in proportion to how far it sits between the two, so a gradient steps through the inks instead of banding into one.

Each pair below is the same 640x360 crop (from 1280x720), before #320 (RGB), after it with the defaults, and after it with `--style ascii:chroma=3,palette_dither=1`:

| `--ascii-palette` | RGB (before) | OKLab | OKLab, `chroma = 3`, `palette_dither = 1` |
|---|---|---|---|
| `cga`, pbr_test | ![](../images/ascii-palette-cga-pbr-test-rgb.png) | ![](../images/ascii-palette-cga-pbr-test-oklab.png) | ![](../images/ascii-palette-cga-pbr-test-dither.png) |
| `cga16`, pbr_test | ![](../images/ascii-palette-cga16-pbr-test-rgb.png) | ![](../images/ascii-palette-cga16-pbr-test-oklab.png) | ![](../images/ascii-palette-cga16-pbr-test-dither.png) |
| `cga`, mesh_meadow | ![](../images/ascii-palette-cga-mesh-meadow-rgb.png) | ![](../images/ascii-palette-cga-mesh-meadow-oklab.png) | ![](../images/ascii-palette-cga-mesh-meadow-dither.png) |
| `cga16`, mesh_meadow | ![](../images/ascii-palette-cga16-mesh-meadow-rgb.png) | ![](../images/ascii-palette-cga16-mesh-meadow-oklab.png) | ![](../images/ascii-palette-cga16-mesh-meadow-dither.png) |

Under `cga16` the red box now takes `#ff5555` rather than `#aa0000`, the cube's top turns cyan, and the meadow's grass is green where RGB made it yellow. Its path, a muted tan, comes out grey with the defaults, and yellow and red with chroma pushed. The meadow's sky stays white or grey under any setting. That is correct: it is `#cbccd0`, close to neutral.

The CPU reference is `Palette::pick(colour, chroma, dither, cell)`, and the shader's is `pal_pick` in `crates/flint-render/src/palette.wgsl`, which the glyph pass includes. Inks are converted to OKLab once a frame on the CPU. A GPU test (`the_palette_matcher_agrees_with_its_cpu_reference`) holds the two to the same ink over a grid of 4096 colours with chroma and dither on. A `luminance` palette ignores both keys. Matching in OKLab costs about 0.03 ms at 1080p for `cga16`'s 15 inks (0.076 ms before, 0.103 ms after, `--gpu-time 240` on an RTX 4070 with edges off). Dither adds about 0.003 ms.

Since #277 split the style into a cell pass and a glyph pass, most colour choices cost nothing extra. On an RTX 4070 at 1080p with edges off, the ASCII pass takes 0.047 ms in `source`, `tint`, `raw` and `mono`, with the `phosphor` ladder (0.048 ms), and with `ground = "black"` or `"inverse"`. All of these are the same as plain ASCII. Two settings cost more. Matching `cga16`'s 15 inks in OKLab takes 0.099 ms, and `glow = 0.6` takes 0.154 ms, because the glyph pass makes 24 more glyph reads a pixel. Before the split these were 0.143 ms (`source`), 0.152 ms (`cga16`), 0.144 ms (`inverse`) and 0.220 ms (`glow`). With the [edge passes](#edges) on, every mode measures 0.36--0.49 ms. The roughly 0.35 ms of edge work varies from run to run by more than the colour modes differ, so it hides them. To measure: `flint render examples/pbr_test.scene.toml --ascii --ascii-no-edges --width 1920 --height 1080 --gpu-time 240`, adding each mode's flag. The figures are the best of 5 alternating processes per mode, with SSAO steady at 0.150 ms across all of them as a control (#383).

### Edges

Luminance alone loses a silhouette wherever an object is about as bright as what's behind it. So cells along the scene's edges draw the glyph set's line glyphs, `─ │ ╱ ╲` in the built-in set, picked by the direction of the edge through the cell (#267). Away from edges cells keep their tone glyphs.

![A sphere, a box and a pillar in ASCII, each outlined in unbroken line glyphs](../images/ascii-edges-on.png)

![The same scene with --ascii-no-edges: the sphere's top half melts into the ground behind it](../images/ascii-edges-off.png)

*`examples/ascii_edges.scene.toml` at 1280x720, cropped at 1:1, with edges (top) and with `--ascii-no-edges`. The sphere and the box carry `style_mask { style = "ascii" }`; the pillar has none, so it is outlined by depth alone.*

Edges come from geometry, never from colour, so a wood texture makes none but a table's outline and its creases do (the ASCII spike's finding, #261). Three passes, before the glyph pass:

1. **Edge pixels**, at the depth's resolution. A 3x3 Sobel over the relative view distance finds silhouettes; one over normals rebuilt from the depth finds creases, counted only where the 5x5 patch is one surface. Where an ASCII-styled entity (an `ascii_style`, or a `style_mask { style = "ascii" }`, [the entity style mask](#the-entity-style-mask)) meets anything else, including another styled entity with different settings, the pixel is an **outline**. Silhouettes and outlines keep their near side only, so each is drawn once.
2. **Cells**, one fragment a glyph cell. It reads every edge pixel in its cell, not a sparse lattice, so a one-pixel outline can't slip between samples, and sums their structure tensors: the dominant orientation picks the glyph. The cell draws a line when it holds at least `edge_threshold` cell widths of edge and at most `edge_max` (an outline needs only the threshold).
3. **The glyph pass** draws the cell's line glyph in the one cell the line passes within half a cell of, measured across it: down the cell's centre column for a line nearer horizontal, along its centre row for one nearer vertical (#352). The cell pass finds where that is from the edge pixels' centroid, each pixel weighted by how much its gradient agrees with the cell's line, so a corner's other arm doesn't pull it. That is the cell a Bresenham line lights, so a line along a cell border, a coarse diagonal or a silhouette at render scale 0.5 draws one chain of cells, not two. A band of edge pixels wider than one (two-pixel stairs at scale 0.5) can straddle the border; the two halves count as one line when they run within 10° of each other and lie within three quarters of a cell, and the pair's centroid decides. Two real edges a cell apart, or converging at an angle like a box's silhouette and the crease below it, each keep their own.

#### Corners

At a corner the outline turns 90° inside one cell, and the summed tensor has no dominant direction: the cell used to pick whichever of the four lines won by a hair, a stray `╱` at a box's top corner. Two more steps fix it (#348):

- The cell pass measures the tensor's **coherence**, (λ1 − λ2) / (λ1 + λ2): nearly 1 for a straight line, (a − b) / (a + b) for two arms at right angles of lengths a and b, 0 when the corner sits in the cell's middle. A cell that draws a line with coherence under `edge_corner` is marked a corner.
- The glyph pass asks the marked cell's four neighbours which **arms** run into it: a `─` beside it, a `│` above or below. The arms pick the glyph: right and down make `┌`, three arms a tee (`├ ┤ ┬ ┴`), four the cross `┼`, and two opposite arms the straight line, which also cleans up a stray diagonal inside a run. Only a neighbour sure of its own direction lends an arm, so two marked cells side by side don't each take the other for one. With fewer than two arms that meet, the cell keeps its line.

```text
edge_corner = 0                       edge_corner = 0.9 (the default)
%%%%%╱──────────╲%%%%%%%%%%%%%%%%%    %%%%%┌──────────┐%%%%%%%%%%%%%%%%%   row 10: the pillar's top
%%%%%│++++++++++│%%%%%%%%%%%%%%%%%    %%%%%│++++++++++│%%%%%%%%%%%%%%%%%
...
%%%%%│++++++++++│%%%%%%%%%%%%%%%%%    %%%%%│++++++++++│%%%%%%%%%%%%%%%%%
─────╱++++++++++╲─────────────────    ─────┤++++++++++└─────────────────   row 30: the horizon meets it
*****│+++++++++│******************    *****│+++++++++│******************
...
*****│++++++│::::───────########──    *****│++++++│::::───────########──
╲****│++++++│:::::::::::────────**    ╲****│++++++│:::::::::::───┬────**   row 37: the box's top edges
╲****│++++++│::::::::::::::│******    ╲****│++++++│::::::::::::::│******           meet its vertical
```

*`examples/ascii_edges.scene.toml` at 1280x720, columns 72 to 105 of `--ascii-text`. These five cells are all that changed in the frame. At the horizon's right the pillar's edge steps a column left below it, so the cell sees no `│` beneath and draws `└`.*

The glyphs come from the set's `[edges] corners` ([glyph sets](glyph-atlas.md#a-glyph-set-file)); the built-in set has all nine. A set without them, like `examples/fonts/halftone.glyphs.toml`, draws the line glyphs it always did (the golden `ascii-edges-no-corner-glyphs` holds that to the pixel against `edge_corner = 0`), and a set with only the four corners keeps its lines at tees. A per-entity variant in a font of its own uses that font's corners. The edge view below draws a cell's corner in yellow, its arms from the cell's centre.

Two corners the coherence test can't see turn too (#637):

- **A line that ends beside a perpendicular one.** When the turn's two arms fall in neighbouring cells, each cell's pixels run one way and neither is marked: a `─` that runs on past the outline sits above the `│` that starts below it. A `─` (or `│`) with its own line on only one side ends in its cell; a `│` above or below it (a `─` beside it) is the turn's other arm, and the cell draws the corner, or the tee with an arm on both sides. Past the end, a perpendicular line that ends there itself counts as an arm, which is how the `│` under a `─` that turned down becomes `┤` when a crease runs into it from the side. A line that runs on through the cell never turns, so lines crossing never become `┼`.
- **A gap.** A cell holding a sliver of edge, under `edge_threshold`, draws no line. Where a `─` runs into it from the side and a `│` from above or below, it draws their corner, or a tee with a `─` on both sides: the hole an outline leaves where it clips the cell's corner, or where a `─` ends in one cell and the `│` starts diagonally past it. Arms count only the way they run into the cell, so the cell diagonally inside a box's corner (a `─` above, a `│` beside) never draws one.

Neither turns a staircase: a line that carries on past its end as a diagonal, or the same line a cell across (a near-vertical edge stepping a column), doesn't end, and a gap with a `│` beside it or a `─` above or below it is left empty. A marked corner whose own arms make no corner draws its line and lends it as an arm. `edge_corner = 0` turns all of it off.

```text
before #637                               after
##########################────────****    ##########################───────┐****   the box's top-right
───####################──────────│****    ───####################──────────┤****   (a crease meets it)
:::───────###──────────**********│****    :::───────###──────────**********│****
::::::::::─*─********************│****    ::::::::::─┬─********************│****   its front corner, a gap
:::::::::::│*********************│****    :::::::::::│*********************│****
```

*`examples/ascii_edges.scene.toml` at 1920x1080, columns 138 to 175 of `--ascii-text`. These three cells are all that changed in the frame.*

Under a render scale the edge pixels are the scene's, so a line is still one cell wide. The editor grid of `flint render` (on unless `--no-grid`) is an overlay drawn after the opaque geometry that writes no depth (#346), so the edges never trace it; its lines still lift the tone glyphs they cross, which is why renders of the look pass `--no-grid`.

Edges come with the colour modes: by default (`edge_ink = "cell"`) line glyphs are inked as the cell's tone glyph would be, cut out of the ink under `ground = "inverse"`, and glow around them. The edge cells lie on the screen grid, so per-entity variants on that grid get them, and a variant on `grid = "object"` keeps its tone glyphs. Edge cells keep no history: under #277's temporal stability the tone glyphs hold still and the lines follow the geometry each frame. The [stylize goldens](#goldens) pin pbr_test without edges (the #265 golden, unchanged), with edges, and the sphere and box above.

#### Edge ink

Along a silhouette a cell's colour is a mix of the object and what's behind it, so an outline inked like its cell drifts between the two and never stands apart from the fill. `edge_ink` gives the lines an ink of their own (#351):

- `cell`, the default: as the cell's tone glyph, above.
- `color`: one colour, `edge_color`, mixed toward the near side's hue by `edge_tint` (the ASCII spike's pair, #261).
- `hue`: the near side's own colour, at the cell's value, inked as the colour mode inks a cell (its hue under `source`, its nearest ink under `palette`; under `tint` and `mono` that is the tint or white again).

The edge cell pass knows the near side: edge pixels keep only the near side of a step, so it averages the picture under the pixels that decided the cell's line and stores their hue beside the glyph. A styled entity's silhouette floor (`outline`, [Silhouettes](#silhouettes)) is an outline too and takes the same ink. Under `ground = "inverse"` the lines are drawn in the edge ink over the ink-filled cell, not cut out of it.

```toml
[post_process.styles.ascii]
color = "tint"            # a green phosphor fill...
edge_ink = "color"
edge_color = "#ffb347"    # ...outlined in amber
```

From the command line, `--style ascii:color=tint,edge_ink=color,edge_color=#ffb347`.

![examples/ascii_edges.scene.toml under a green tint: edges in the cell's ink (left) and in amber (right)](../images/ascii-edge-ink.png)

*`examples/ascii_edges.scene.toml` at 960x540, cropped and doubled: `color = "tint"` with `edge_ink = "cell"` (left) and `edge_ink = "color", edge_color = "#ffb347"` (right).*

#### Seeing the decisions: `--debug-mode edges`

A missing or doubled line glyph could come from any of the three passes, and the glyphs alone don't say which. The edge view (#349) draws what each pass decided, in place of the glyphs, over the scene in dim grey:

```bash
flint render examples/ascii_edges.scene.toml --debug-mode edges --no-grid --output edges.png
flint render examples/ascii_edges.scene.toml --debug-mode edges --no-grid --style ascii:cell=16 --output cells.png
```

- **Edge pixels**, by the source that marked them: cyan a depth step, amber a crease, magenta a styled entity's outline.
- **Each cell's decision**: a green line in the glyph's direction where the cell draws one; a yellow corner or tee, its arms from the cell's centre, where it draws one of those ([corners](#corners)); a dim dashed line where it holds a line that runs more than half a cell from its centre, which a neighbour draws (pass 3's thinning); a red wash where it holds more than `edge_max` and counts as texture; nothing where it holds less than `edge_threshold`.
- **Each cell's edge length**: a bar along the cell's foot, full at `edge_max`, with a white tick at `edge_threshold`. In cells 12 pixels or wider the length is also written in the top left, in cell widths.

![The ASCII edges demo in the edge view: cyan silhouettes on the pillar and the horizon, amber creases on the box, magenta outlines around the sphere and the box, green line glyphs through the cells, and the legend card in the lower left](../images/ascii-edges-debug.png)

![A close-up at 16-point cells: the pillar's foot and the box's top, each cell's length written in its corner](../images/ascii-edges-debug-cells.png)

*`examples/ascii_edges.scene.toml` at 1280x720 (top) and at `ascii:cell=16`, cropped and doubled (bottom). The horizon is a depth step, the sky's far plane against the ground. Along the box's top the crease and the outline run a few pixels apart, so its cells hold 2.1 to 2.7 cell widths, inside `edge_max`; where the pillar meets the ground they hold 3.0, right at it.*

Read it beside the four keys in the table above: `edge_depth` and `edge_normal` decide the coloured pixels, `edge_threshold` and `edge_max` the cells. `flint render` prints the thresholds it drew at and the legend, and draws the same legend in the image's lower left. Lower `edge_max` until a cell turns red to see how close a run of geometry is to being called texture. The view draws through post-processing, and switches ASCII on at its defaults in a scene that has none. In the player and viewer it is **Style Edges** in F4's debug shading list, with the legend under it.

### Dark props: exposure and normalize

Glyph density follows luminance, so a dark prop maps to `.` and `-` and nearly disappears: a barrel in the meadow reads as a black shape with a few dots. Carrying hue in the ink helps, but the brightness is still gone. A `luminance` palette (`phosphor`, `amber`, `mono`) dims it twice, since it picks the ink rung by the same tone (#289).

Two settings fix it, on the scene's table or on one entity's `ascii_style`:

- **`exposure`**, in stops: the cell's colour times 2^exposure in linear light, before the glyph and ink are picked. Simple, but a dark prop's range stays narrow, only higher up.
- **`normalize`**, 0 to 1: the style's own tone range is stretched over the whole ramp. Each style id has its own range: the whole frame's (slot 0), and each per-entity variant's, so a dark barrel and a bright sign in one frame are stretched separately. 1 maps the range onto 0..1; between 0 and 1 mixes the stretched tone with the plain one.

```toml
[entities.barrel.ascii_style]
normalize = 1.0     # its own tones over every glyph of the ramp
exposure = 0.5      # optional: half a stop first
```

![The meadow barrel four ways: smooth; plain glyphs, mostly dots; brightness 0.35 and contrast 1.4, brighter but still narrow; normalize = 1, the full ramp from dots to @ with the hoops as rows of = and -](../images/ascii-normalize-barrel.png)

*The barrel in `examples/ascii_entities.scene.toml` at 1280x720, cropped and doubled: smooth, plain, the `brightness = 0.35, contrast = 1.4` it needed before, and `normalize = 1` (its setting now). Its lower half averages 0.024 luminance in the smooth render, so it stays sparse even stretched: that is real darkness, not lost detail.*

![Three barrels: plain, exposure 1.5, normalize 1](../images/ascii-dark-props.png)

*`examples/ascii_dark_props.scene.toml`: plain, `exposure = 1.5` and `normalize = 1`, side by side.*

The range is measured from the same frame, so `flint render` stays deterministic and nothing lags a frame behind: three small passes before the cell pass walk a fixed 320x180 lattice over the picture, sum each slot's samples (count, luminance, luminance squared) in runs of 20, add the runs into rows, and add the rows into each slot's range: its mean -/+ 2 standard deviations. The range maps linearly onto 0..1, then a gamma (0.35 to 2.5) puts the slot's mean at mid-grey, since a lit top and a shadowed side are often both in range and the linear stretch alone leaves the shadow blank. That is a percentile without a histogram: a highlight or a crack moves it far less than it would a min and max. A range narrower than 0.08 is widened around its middle, so a flat wall's noise doesn't fill the ramp. The passes run only when some slot asks for `normalize`; brightness and contrast then apply to the stretched tone as before. The statistics cost about 0.02 ms on an RTX 4070 at 1080p (`examples/ascii_entities.scene.toml` with `--ascii-edges 0 --gpu-time 240`: 0.094 ms with the barrel normalised, 0.074 ms without). About half of that is the three passes' fixed cost. Before #337 the lattice rows were summed by one fragment each, 180 threads walking 320 samples apiece, and the statistics cost about 0.075 ms.

## Per-entity ASCII

An entity with an `ascii_style` component is drawn in glyphs while the rest of the scene stays smooth (#266). No `[post_process.styles.ascii]` table is needed:

![examples/ascii_entities.scene.toml: the meadow and the ground stay smooth while the bench (amber blocks), the skinned YBot (its own colour), the barrel (on its own grid) and the crate with its lantern (phosphor green) are glyphs; the lantern's glass box stays smooth](../images/ascii-entities.png)

*`flint render examples/ascii_entities.scene.toml --no-grid --target 0.7,0.7,4.2 --distance 5.2 --yaw 29 --pitch 10`, 1280x720.*

```toml
[post_process.styles.ascii]   # optional: the preset every styled entity starts from
scope = "entities"            # without it, the table would glyphize the whole frame
background = 0.3

[entities.bench.ascii_style]  # only what differs from the preset
ramp = "blocks"
color = "tint"
tint = [1.0, 0.70, 0.30]      # a colour, as in the scene's table (or "#ffb34d")
levels = 4

[entities.ybot.ascii_style]   # empty: the preset as it is (skinned meshes work too)

[entities.lantern_glass.ascii_style]
enabled = false               # stays smooth, even under a whole-frame ASCII
```

- **Fields.** Every key of the table above except `scope`, all optional. A field the entity leaves out comes from the scene's table, then the style's default. The schema gives no field a default on purpose, since the loader would fill it in and hide the scene's setting. An archetype's `[archetype.<name>.defaults.ascii_style]` is a preset too, and an entity's own fields merge over it field by field.
- **Descendants** inherit their ancestor's style until one carries an `ascii_style` or `style_mask` of its own; the nearest wins. A plain `style_mask { style = "ascii" }` is the same as an empty `ascii_style`.
- **Ids.** Entities with the same settings share one style mask id. Each distinct set of settings is a variant, and the pass looks up each pixel's settings by its id. The variant table is two storage buffers, every variant's row and each mask id's slot, sized to the frame: they start at 64 entries and double when a frame needs more, so 500 props on `grid = "object"` each get a row (#336). A pixel of the whole frame never reads them, so full-screen ASCII costs what it did. Past 4095 variants (the tone statistics' texture holds a column each) the rest are drawn like the whole frame, with a warning.
- **Colour.** A cell's colour averages only the samples of the pixel's own variant, so a styled prop's glyphs carry its colour, not the meadow's behind it. Its silhouette is the mask's, to the pixel. With a `footprint` of 2 or more those 16 samples are summed once a cell, in a pre-pass (#313, `ascii_variant_cells.wgsl`), rather than in each of the cell's pixels. Variants on one grid (the same cell, origin and footprint) share a band of rows, and each cell keeps the first two of the band's variants met in it. A pixel of a third variant still sums for itself, and so does a variant past 8 grids. At footprint 1 every pixel sums for itself: its 16 loads hit the 16 texels its cell shares, so they come from the texture cache, and there a pre-pass over every cell cost more than it saved. The pixels are the same either way, to the bit. Timings are in [Profiling](../guides/profiling.md#gpu-timing).
- **The whole frame.** With the scene's table on and `scope = "screen"` (or `--ascii`), everything is glyphs and entities still change their own settings; `enabled = false` keeps one smooth.
- **Grids.** By default every styled entity shares the screen's grid, which keeps glyphs aligned where props meet (the spike's finding, #261). `grid = "object"` pins an entity's cells to its projected origin: its glyphs hold still while it slides across the screen, but still swim when it turns or comes closer (#277). Each such entity gets a variant of its own.
- **Colour modes** (`color`, `palette`, `ground`, `glow`) are per entity too: a frame packs up to 32 palette inks in all, and a palette that doesn't fit beside the others uses the scene's, with a warning.
- **Edges between entities** come from [edges](#edges) (#267). Without them a styled prop's edge is only where its glyph field stops, so two styled props that overlap would run together. The edge passes draw line glyphs along depth steps and creases, and an outline wherever the style mask changes from one of the style's ids to any other: the plain `ascii` id or any variant's, so a change between two variants is an outline too. Ids are per variant, not per entity, so two props with the same settings that touch on one surface are told apart by the depth and crease edges, not the mask. Variants on `grid = "object"` don't draw edges yet, because the edge cells are on the screen grid.
- **Temporal stability** (#277's `smoothing`, `hysteresis` and `dither`) applies to the whole frame's cells. A per-entity variant is drawn per pixel on its own grid and keeps no history yet, so a styled prop can still shimmer in motion.
- **From scripts** (#269): `set_entity_style(id, "ascii")`, `set_style_param(id, "cell", 12.0)` and `clear_entity_style(id)` edit the component at runtime, and changing a param every frame makes no GPU allocation. `tween_style_param(id, "cell", 7.0, 1.5)` lets the engine ease one (#370), and `get_entity_style(id)` / `get_style_param(id, "cell")` read back what an entity is drawn in, inherited or whole-frame (#372). See [Scripting → Style API](scripting.md#style-api) and `examples/ascii_story.scene.toml`. Whole-frame styles have their own calls, `set_post_style` and `set_post_style_param` (#369; [Scripting → Whole-frame styles](scripting.md#whole-frame-styles)).
- **Fonts** (#335). An entity's `font` names its own glyph set, so props can draw in different fonts in one frame. Every set in use is stacked into one atlas texture (`GpuGlyphStack`, uploaded again only when the frame's sets change), and each variant's row carries its font's glyph cell and where its ramp and its four edge glyphs sit, so the cell's height follows that font's aspect and the edges are drawn in its own line glyphs. A frame holds up to 8 fonts and an atlas stack up to 8192 pixels tall. A font that doesn't fit, or whose ramp and edges don't fit in the frame's 128 ramp steps, draws in the scene's font, with a warning. `examples/ascii_fonts.scene.toml` draws a YBot and a barrel in a hand-drawn halftone set (`examples/fonts/halftone.glyphs.toml`, 8x12 cells) beside props in the built-in letters.
- **Not yet:** ramps beyond 128 steps in all across the frame's variants (a ramp that doesn't fit uses the scene's, with a warning).

```bash
flint render examples/ascii_entities.scene.toml --output ascii.png              # as authored
flint render examples/ascii_entities.scene.toml --ascii-scope screen            # the whole frame
flint render examples/pbr_test.scene.toml --ascii --ascii-grid object           # flags set the preset
```

At 1080p on an RTX 4070 the demo's ASCII pass costs about 0.045 ms and its mask 0.013--0.023 ms (DX12, Vulkan); with `--ascii-scope screen` the pass costs about 0.06 ms, and with no styled entities 0.04 ms. In Chrome it is about twice that, and a render scale leaves it unchanged, since the cells are output pixels. A phone is another matter: on a Pixel 8 Pro (Mali-G715) any ASCII, per entity or full screen, with or without edges, takes the web player from about 64 fps to 25--30, while `invert` and `palette` cost nothing visible (#378; the pass is being found in #568). The numbers by backend, scale, browser and phone are in `docs/design/web.md` ("Stylize passes", #270, #378). It works under `--msaa 4` and `--render-scale`, where the silhouette follows the scaled mask. F4 → Styles → ASCII offers `scope` and `grid` beside the other settings, and `flint edit --watch` hot-reloads `ascii_style` edits.

### Silhouettes

A styled entity's silhouette is cut to the pixel, but it is filled with cells 8 points wide. A bench leg 3 px wide holds a sliver of one glyph, mostly ground, so under per-entity ASCII the meadow bench's legs and rails were dark smudges, while the whole frame kept the bench readable as a shape cut out of the glyphs around it (#261, #288). Two ink floors keep thin features:

- **`outline`** (points, default 1) inks the entity's own pixels that lie within that distance of a pixel of another mask slot. The silhouette is drawn as a line inside the entity, in its own ink (the hue at full value, a tint, or a palette ink, as its glyphs are), and a feature two outlines wide comes out solid.
- **`min_coverage`** (default 0.5) is the least ink of every pixel in a cell the entity only grazes: under a quarter of the cell's colour samples, easing out by five eighths.

![examples/ascii_silhouette.scene.toml, the bench at 14 m: before #288 its legs are black lumps; with the outline and minimum coverage the bench reads in its own wood-coloured ink](../images/ascii-silhouette-bench.png)

They apply to per-entity variants only: the whole frame's cells have no silhouette, and `ground = "inverse"` already fills its cells with ink. `outline = 0, min_coverage = 0` draws what came before. The outline reads up to 4 rings of 8 mask texels for each styled pixel. On the demo at 1080p on an RTX 4070 (`--gpu-time 240`, edges on) the ASCII pass measured 0.38--1.0 ms with the floors on and 0.53--1.0 ms with them off: run-to-run noise hides the difference.

```bash
flint render examples/ascii_silhouette.scene.toml --no-grid -o bench.png
flint render examples/ascii_silhouette.scene.toml --no-grid --style ascii:outline=0,min_coverage=0 -o before.png
```

### Whole-cell silhouettes

The outline reads a prop as a shape cut out of the scene: the glyphs end at the mask's pixel-exact edge, so the cells along it are half glyphs. `silhouette = "cell"` (#338) is the other reading, a silhouette made of whole glyphs. Each cell of the variant's grid is all glyph or all scene, decided by how much of it the entity covers:

- A cell where the entity covers at least `cell_share` (default 0.5) of the cell's 16 colour samples is the entity's. Every pixel in it draws the entity's glyph, the scene's pixels in the cell included.
- A cell it covers less of is dropped. The entity's own pixels in it show the scene around the entity, each the picture at the nearest of the cell's samples that isn't the entity's, rather than the prop's smooth shading. With the whole frame in ASCII they draw the whole frame's glyph instead.

![examples/ascii_entities.scene.toml, the barrel and crate at 3x: with pixel silhouettes the glyphs end at an outline along the mask's edge; with cell silhouettes every edge cell is a whole glyph or the meadow](../images/ascii-silhouette-cell.png)

The edge becomes a staircase of cells, like a character on a text screen. The pixel `outline` is off on a snapped variant, since the edge is the cells'. `min_coverage` still applies, and lowering `cell_share` (0.25 keeps more of a bench's legs) is the way to hold thin features. Edge glyphs (#267) still draw in the cells the entity keeps. Object grids (`grid = "object"`) snap on their own grid. A variant that keeps `silhouette = "pixel"` keeps its own pixels even inside a snapped neighbour's cell.

```bash
flint render examples/ascii_entities.scene.toml --no-grid --style ascii:silhouette=cell -o cells.png
flint render examples/ascii_entities.scene.toml --no-grid --style ascii:silhouette=cell,cell_share=0.25 -o thin.png
```

The glyph pass works out each pixel's cell, so every pixel of a cell gets the same answer. It reads the cell's 16 mask samples once for the pixel's own grid, then once for each other grid a snapping variant uses (up to 8; a variant on a ninth still drops its own minority cells but fills none). Each pass keeps counts in registers for the first two snapping variants met in the cell, so a third variant of the same grid meeting them in one cell is left out. On `examples/ascii_entities` at 1080p on an RTX 4070 (`--gpu-time 240`, three alternating runs, two snapping grids) the ASCII pass took 0.34–0.38 ms with cells against 0.17–0.24 ms with pixels (edges off), and 0.64–0.68 ms against 0.49–0.54 ms with edges on: about 0.15 ms more. A first version that kept the samples in private arrays cost 0.4 ms more. A pixel the mask itself misses (a grazing silhouette pixel its depth tolerance drops) still passes through, as it does with `pixel`.

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
tone = "picture"       # picture | light: hatch the dark paint, or only where the light falls off
contour = 1.0          # outline strength; contour_width in points
depth = 1.0            # outline sensitivity to depth steps; crease to creases
grazing = 0.0          # 1 spares a plane seen edge-on: the floor inks one horizon, not a band
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
flint render examples/stylize_props.scene.toml --no-grid --style pencil -o page.png
flint render examples/pencil_sketch.scene.toml --no-grid -o props.png        # per-entity variants
flint render scene.toml --style pencil --sweep pencil.direction=surface,screen,tensor -o dirs.png
```

![The pencil sketch demo drawn as a whole page: the ground and sky in graphite, the props keeping their own pencil settings (coloured cask, blue YBot)](../images/pencil-sketch-page.png)

How it draws:

- **Hatching builds tone the way a hand does.** Each layer switches on as the tone darkens past its threshold, and its strokes thicken from nothing rather than popping in. A layer crosses the one before it off the square (at 0°, +65°, −40° and +25°), so layers never make a grid. Each stroke is a dash of its own length that tapers at both ends, is jittered off its row and is bent by a slow wobble. A few dashes are left out.
- **Stroke direction.** `surface` runs strokes along the surface's iso-depth lines, taken from the gradient of 1/z. That gradient is constant over a plane, so a box face hatches straight, and it turns smoothly round a barrel and across a floor. `screen` hatches everything at `angle`. `tensor` follows the picture's luminance edges, a structure tensor blurred over about three stroke spacings in two separable passes at half resolution (#392). In that blur each pixel's direction is weighted by its contour strength, so silhouettes and creases steer the flow and a flat, noisy patch (soft shadow, ambient occlusion) barely counts. The strokes sweep round a shadow's edge instead of fraying into short crossing patches. Where a surface or tensor direction is unsure (a face turned square to the camera, a flat colour), `angle` shows through. Directions are quantized to 15° bins with neighbouring bins blended, so strokes stay straight within a patch, as hand hatching does.
- **Contours** come from depth steps (drawn on the near side), creases (the normal rebuilt from depth turns), the edge of a styled entity's mask, and luminance edges by `detail`. They are read back through a noise offset (`wobble`), twice when `sketchy` is set. A floor seen edge-on steps in depth from row to row as much as a silhouette does, so near the horizon it inks a thick band; `grazing = 1` keeps a depth step only where the second difference of 1/z (zero across any plane) says the neighbours lie on another surface, and the floor draws one horizon line (#404). No crease is drawn within two pixels of the frame's edge, where the neighbours are clamped (before #404 every pencil look outlined the frame).
- **Tone from the light** (#869). The picture's luminance is albedo times light, so by default (`tone = "picture"`) a dark material is hatched as if it were in shadow, and a white prop in deep shadow stays bare. `tone = "light"` divides the picture by the [style albedo](#the-style-albedo) (each opaque mesh's own colour, drawn only while a row asks for it), both in display values, so the hatching follows where the light falls off whatever the paint. On the plain bench at `brightness = 0`, picture tone hatches the whole brown floor; light tone leaves it blank and hatches the barrels' undersides, the crate's shaded face and the shadows they cast. On the darker `examples/ascii_dark_props` ground, where the `rotoscope` look's own `brightness = 0.5` still hatches everything, light tone at 0 hatches only the barrels' shadow. So one `brightness` holds across scenes. Where no mesh was drawn (the sky, terrain, grass, water, particles) or the paint is nearly black, the picture's own luminance stands. Per entity too (`pencil_style { tone = "light" }`).

```bash
flint render scene.toml --no-grid --look rotoscope --style pencil:tone=light,brightness=0 -o shadows.png
```

![The rotoscope look at brightness 0 on stylize_props_plain: tone = picture hatches the whole brown floor; tone = light leaves it blank and hatches only the shadows](../images/pencil-tone-light.png)

- **Line only.** `layers = 0` with `shade = 0` draws contours on paper and nothing else, and skips the stroke-direction work, so it is the cheapest pencil.
- **Deterministic.** Every random choice is an integer hash of the stroke's row, dash, layer and `seed`, so the same seed draws byte-identical frames. `boil` moves the seed in whole steps of post time, which `--style-time` fixes.

![The three stroke directions on stylize_props](../images/pencil-directions.png)

![Close up: the crate's faces hatched straight, its outline doubled, the ground cross-hatched in three layers](../images/pencil-closeup.png)

### Paper and strokes from files

A scanned paper and a strip of real pencil strokes can take the place of the procedural paper and the drawn dashes (#390):

```toml
[post_process.styles.pencil]
paper_image = "cold_press"   # papers/cold_press.png beside the scene, or any path
paper_scale = 1.0            # points one texel covers; the image tiles
stroke_image = "graphite"    # strokes/graphite.png
stroke_rows = 4              # strokes stacked in the strip, top to bottom
```

```bash
flint render examples/stylize_props.scene.toml --no-grid \
  --style pencil:paper_image=cold_press,stroke_image=graphite,stroke_rows=4 -o page.png
```

![stylize_props at 1:1: procedural paper and dashes, the cold-press image, the graphite strip, and both](../images/pencil-images.png)

- **Found like glyph sets**, by [the one asset search](assets.md#where-the-engine-looks-for-a-file) (kinds `papers` and `strokes`): `papers/<name>.png` (then `.jpg`, `.jpeg`) or `strokes/<name>.png` beside the scene, in each directory above it up to the project root, then in the mounted engine; a name with a folder or an extension (`scans/linen.jpg`) is a path tried from each of those directories. The working directory is not searched. Files are read through the vfs, so `flint bundle` carries them to the web. F4 lists the images in every `papers/` and `strokes/` the scene can see. A name that can't be found or decoded warns once, with every path tried, and draws procedurally. Natively an edited file reloads while the viewer is open, as palettes do.
- **The paper.** Its luma, measured against the image's own mean and spread, is the paper's height. The height shades the paper and is the tooth that breaks the strokes, as strongly as `grain` says. So any scan, dark or light, faint or deep, comes out at the same strength. The scan's overall colour cast is taken out when it loads, so the `paper` colour still decides the paper's colour. The image adds relief and local flecks of hue, not a second tint. It tiles from the screen's corner in points, so the paper holds still as the camera moves.
- **The strokes.** The strip is `stroke_rows` strokes stacked top to bottom, each drawn left to right. A dash takes one of them by its hash and stretches it along its length. The row spans twice `thickness`, and the stroke grows from thin as the tone passes the layer's threshold. Ink is the strip's dark or opaque texels (`alpha x (1 - luma)`) over the strip's own 99th percentile, so dark on white and dark on transparent both work, and a light scan still draws a full line. The strip's own ends replace the drawn taper. It works on every `grid`: under `grid = "world"` the row is twice the stroke width in metres, as the world strokes are measured.
- **One paper and one strip a frame.** These come from the scene's table, or else from the first `pencil_style` entity that names one. An entity that names a different file draws procedurally and gets one warning.
- **No sampler.** Images are `Rgba8Unorm` with a mip chain made on the CPU, and the shader reads them with `textureLoad` at the level it picks. Wrapping the paper is integer arithmetic, and the DX12 sampler heap rule holds. `crates/flint-render/src/stylize/images.rs` holds the loader and `style_image.wgsl` the reader, for any style that wants an image.
- `examples/papers/cold_press.png` and `examples/strokes/graphite.png` are synthesized by `tools/style-images/make_images.py` (`python tools/style-images/make_images.py examples`; numpy and Pillow), not scanned. A real scan drops in the same way. With no image named, pencil draws exactly what it drew before: its goldens pass unblessed.

It runs at the display stage, at output resolution, with sizes in points (times the pixels-per-point scale), in two passes. The first pass writes each pixel's tone, contour strength and stroke direction into a scratch target. The second blurs the direction into a flow and draws the strokes. Only when a variant hatches by `tensor`, two half-resolution passes between them blur its flow first (`pencil_flow.wgsl`), and the drawing pass reads the result. At 1080p on an RTX 4070 the whole frame costs about 0.49 ms; `tensor` adds about 0.08 ms (0.56 to 0.64 ms on `examples/pencil_sketch.scene.toml`, the mean of six alternating `--gpu-time 240` runs). With only two props asking for it the cost is 0.19 ms, because both full-screen passes still run and only return early on unstyled pixels.

**Strokes that hold to the surfaces.** Hatching is laid in screen space by default, so when the camera moves the strokes stay where they are and the surfaces slide under them (the shower-door effect). `grid = "world"` (#389) lays the strokes on the surfaces instead. Each pixel's world position is rebuilt from the depth and hatched on the three axis planes (triplanar, weighted by the normal the first pass rebuilds), with stripes measured in metres, so the strokes travel with the props as the camera orbits. A fixed spacing in metres would crowd into grey with distance, so spacing comes in octaves: level *k* puts its rows 2^*k* metres apart. Each pixel picks the octave whose spacing on screen is nearest `spacing` and cross-fades it with the next one, as a mip chain does. The on-screen spacing is measured per plane and per stroke direction, from the quad's derivatives, so a floor seen edge-on climbs to wider rows across its depth. Strokes keep their size on screen at any distance, and each one stays where it lies. Every even row of one octave is a row of the next, and a row is hashed by its odd part and the octave that gives it, so a stroke keeps its dashes as it climbs. `angle` turns the strokes within each plane. `direction` isn't used, and the sky keeps screen hatching. The paper and the contours' wobble stay on the page, as a real sheet would.

```bash
flint render examples/pencil_sketch.scene.toml --no-grid --style pencil:grid=world,scope=screen \
  --frames 72 --frame-dt 0.0333 --orbit 25 -o orbit.png
```

![The barrel at three points of an orbit: screen hatching (top) stays fixed while the barrel turns under it; world hatching (bottom) turns with the barrel](../images/pencil-world-orbit.png)

Measured on that orbit (block matching each textured patch of a plain render between consecutive frames, then asking whether the pencil frame matches the next one better standing still or moved with the surface; 2,244 patches): screen hatching correlates 0.78 standing still and 0.59 moved with the surface, while world hatching correlates 0.59 still and 0.78 moved. Patches that include a silhouette move with it in both modes. At 1080p on an RTX 4070 the whole-frame pencil pass costs 0.52 ms with a world grid, against 0.46 ms on the screen. A prop that moves in the world (not the camera) still slides under world strokes, since they are in world, not object, space.

**Per entity.** Pencil works per entity in the same way as ASCII. A `pencil_style` component, or a plain `style_mask { style = "pencil" }`, draws that entity in pencil while the rest of the frame stays as it is, and each distinct set of settings becomes a variant with its own mask id. Every field of the table above except `scope` can be set on the component, and a field it leaves out comes from the scene's table, then the default. `grid = "object"` pins an entity's hatching to its projected origin, so its strokes travel with it (under translation; `grid = "world"` holds under any camera move). With `scope = "entities"` the scene's table is only a preset. A pixel that another style owns (an ASCII prop in a pencil frame) is left to that style.

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

![examples/pencil_sketch.scene.toml: six props, each its own hand; the world around them stays painted](../images/pencil-sketch-entities.png)

### Motion on twos

Rotoscoped film holds each drawing for two or three frames, and its lines redraw on the same beat. A `boil` redraws the lines that many times a second, but on its own the animation under them still moves smoothly, so the line redraw and the motion read as two separate things. `motion_hold` (#405) draws an entity's position and pose on ticks too:

```toml
[entities.dancer.motion_hold]
rate = 12.0              # drawings a second: on twos at 24 fps

[entities.dancer.pencil_style]
boil = 12.0              # the lines redraw on the same ticks
```

![Eight frames of examples/motion_hold.scene.toml captured by flint-player at --frame-step 1/48: the pendulum, the YBot and the pencil lines change on frames 0 and 4 and hold on the three frames after each](../images/motion-hold-strip.png)

- **What is held.** The component holds the entity and everything below it: the world matrix it is drawn with and a skinned model's bone palette, kept from the first frame of each tick until the next tick. The nearest `motion_hold` up the hierarchy wins, so `rate = 0` on a child lets that branch move smoothly.
- **What it carries is held too** (#873). A point or spot light on a held entity, or below one, shines from where that entity was drawn when the tick began, so the pool of light on the floor steps with the drawing. A directional light has no position to hold: its `direction` is written in world space and doesn't turn with the entity. An emitter (`particle_emitter` or `particle_effect`) on a held entity emits from the held drawing. Its particles are born there, not along the jump to the next drawing, so a trail comes out as beads, one cluster a drawing. Emission over distance still counts the whole way travelled. Once born, a particle moves smoothly: it is an effect, not a drawing. The emitter's velocity, which `inherit_velocity` adds, is the world's smooth one, not a jump each tick.
- **What stays smooth.** Live particles, the ocean, the sky, grass and the camera.

![examples/motion_hold.scene.toml in flint-player at 1.0 s: the held pendulum's lantern leaves a bead of soot for each drawing, the smooth one a continuous streak; both pools of light clear the hatching on the floor below](../images/motion-hold-lanterns.png)

- **Only the drawing is held.** Physics (fixed at 1/60 s), IK and scripts read the world, which goes on moving smoothly, so a held crate still falls and collides at 60 Hz and a held hand still reaches its IK target. In `examples/motion_hold.scene.toml` two pendulums swing as mirrors of each other, one held and one smooth. Captured in `flint-player` at `--frame-step 1/48`, every frame on a tick is byte-identical to the same scene with no hold.
- **Same tick as the lines.** A tick is `floor(time x rate)` of the post time, the clock `boil` reads. Pencil's boil steps its seed through the same function, so at `boil = 12` and `rate = 12` the pose and the lines change in exactly the same frames. A tick lands a thousandth of a tick early, so a clock that sums 1/60 steps to a hair under 1/12 still turns on frame 5.
- **The whole frame.** `[post_process] motion_hold = 12` (or `--motion-hold 12` on `flint render`, `flint play` and `flint-player`) holds every entity that names no rate of its own, with the lights and emitters they carry. It also holds the post time that styles, film grain and the tear read to the start of each tick, so a whole-frame pencil boiling at 24 still redraws once a tick.
- **Scripts.** `set_motion_hold(id, rate)`, `clear_motion_hold(id)` and `get_motion_hold(id)` write and read the component directly, so a hold set in `on_update` draws from that frame.
- **Deterministic.** The host stamps the renderer with the frame's game time (`SceneRenderer::set_motion_time`) before it uploads bones: the player's clock, a capture's moment. It stamps the particles with the same seconds (`ParticleSync::set_motion_time`) before they step, so an emitter steps on its carrier's frames. A `--frame-step` capture is byte-identical run to run. `flint render --time 0..2 --fps 24` draws it. A renderer that is never stamped holds nothing.
- **In the editors** (#871). The model previewer (`flint edit model.glb`) holds on the clip's own time, the one its timeline shows: playing, the pose changes `rate` times a second of clip time (fewer a second of wall time when the clip is slowed, as slowing film does), and a paused scrub steps through the same drawings, the panel naming which one. Its **Hold** switch and rate, or `--motion-hold 12`, put it on. A layer dial, a posed bone or a clip switch shows at once and is held from there. The scene viewer (`flint edit scene.toml`) holds on its own wall clock, from the scene's `motion_hold` components, its `[post_process] motion_hold` or `--motion-hold`. The viewer plays no animation and runs no physics, so a held YBot stands in its bind pose and the pendulums hang still; what steps there is what you move: drag a held entity with the gizmo and its drawing follows on the ticks, then lands where you drop it.

```bash
flint render examples/motion_hold.scene.toml --no-grid --anim-time 0 --time 0..2 --fps 24 -o twos.gif
```

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
    tween_style_param(barrel, "blend", 1.0, 3.0);   // the engine eases it in (#370)
}
```

A tween is the engine easing the blend for you ([Scripting → Tweens](scripting.md#tweens)); `set_style_param(id, "blend", k)` in `on_update` still works for a curve of your own.

![The barrel dissolving into green phosphor glyphs at blend 0, 0.25, 0.5, 0.75 and 1: its cells turn whole, in ragged patches that spread](../images/ascii-blend-dissolve.png)

![The bench cross-fading into amber blocks at blend 0, 0.25, 0.5, 0.75 and 1: every pixel a mix of its wood and its glyph](../images/ascii-blend-fade.png)

*Frames from `flint-player examples/ascii_reveal.scene.toml --frame-step 0.05 --screenshot-every 0.05`, cropped and doubled.*

- **`fade`** mixes every pixel of the entity with its glyph pixel by `blend`, in display space.
- **Every other mask-reading style fades too.** Pencil, watercolor and user styles get `blend` from the framework's [cross-fade](#blend-for-free) (#396): `pencil_style { blend = 0.5 }` is half sketch, half the prop's own shading, in linear light. Only ASCII has a `transition` of its own.
- **`dissolve`** turns whole cells: a cell is a glyph once `blend` passes its place in a dissolve order, half patch noise a few cells across and half per-cell hash, so glyphs arrive in ragged patches that spread. The order is fixed per cell, so a ramp never shimmers, and on `grid = "object"` it travels with the prop.
- **A prop turns `blend` of its own cells (#398).** A small prop holds only a few of the noise's patches, so on the order alone a 6x5-cell prop at blend 0.5 turned 30--73% of its cells, depending on where it sat. So each cell is compared by its *rank* in the order among the same variant's cells within 8 cells of it: two short passes before the glyph pass (`ascii_dissolve_order.wgsl` marks each cell the variant holds a lattice sample of and writes its order; `ascii_dissolve_rank.wgsl` counts the lower ones in a 17x17 window). A prop up to 9 cells across and down turns exactly round(`blend` x its cells) wherever it sits; a larger one is ranked window by window, which keeps its patches and flattens the noise's slow swells. The patches are the same ones as before. As a prop moves on the screen grid and its cells come and go, about one cell near the threshold may change with each. Up to 8 dissolving variants are ranked (`MAX_DISSOLVE`); past them a variant falls back to the order itself. Measured from `flint render` on `tests/fixtures/scenes/stylize_blend.scene.toml`'s barrel at 17 placings, about 40 cells: at blend 0.5 it turned 49--74% of its cells before, and 44--57% after. The rest of the spread is cells the 16 lattice samples miss, such as outline slivers. On an RTX-class GPU the two passes stayed under `--gpu-time`'s noise with the barrel filling half of a 1080p frame.
- **It rides in the mask, not the variant.** The [style mask](#the-entity-style-mask) carries each pixel's blend in its `.g` channel beside the id in `.r`. Two props fading at different rates still share one id, and a ramp makes no new variant, row or GPU object (a GPU test in `tests/stylize.rs` ramps it and counts). Like the id, the blend is the styled root's, inherited by its subtree.
- **`style_mask { blend = ... }`** works too, for a style with no component of its own. A style component's `blend` wins over the `style_mask`'s on the same entity.
- **Under a whole-frame style**, a prop at blend 0 is a smooth hole in the glyph field: the blend says how much of the entity's *own* variant shows, and below 1 the rest is the picture as it came.
- The `style` debug view draws each entity's hue as strong as its blend.
- **Cost.** The blend costs one more channel in the mask (`Rg16Uint`, 4 bytes a pixel) and no extra texture read. Pixels a blend leaves plain skip the glyph work: at 1080p `tests/fixtures/scenes/stylize_blend.scene.toml`'s ASCII pass took 0.31 ms, against 0.39 ms for the same props fully styled.
- **At blend 0 a style costs nothing** (#400). When every entity drawn in a per-entity style is at blend 0 (a blend that rounds to 0 in the mask's encoding counts), the style counts as off: neither its pass nor the mask is drawn, and the frame is the unstyled one, byte for byte and at the unstyled scene's GPU time. A scene table with `scope = "entities"` doesn't keep it on, because it is only the entities' preset. The ids don't change, so a ramp out of 0 finds its id and variant where it left them. What starts again is anything a style keeps from frame to frame, as after a camera cut, since its pass skipped frames; nothing it held was visible. A whole-frame (screen) table keeps the style on at any blend, because there the props at 0 are holes in a styled frame.

`examples/ascii_reveal.scene.toml` is a 10-second loop: the barrel dissolves into phosphor glyphs, the bench fades into amber blocks, and both go back. The `ascii-blend` golden pins a half-way dissolve and a 40% fade.

### Blending a whole-frame style

Every style's table also takes `blend` (#369): how much of the style's result shows over the picture it read, from 0 (none) to 1 (all of it, the default). It is the framework's, not the style's, so Kuwahara, watercolor, palette, the full-screen ASCII look and user styles all take it with no code of their own:

```toml
[post_process.styles.kuwahara]
radius = 6
blend = 0.5               # half paint, half the scene as it was
```

```bash
flint render examples/dream_story.scene.toml --style kuwahara:radius=7,blend=0.5
```

A script ramps it to cross-fade the whole world into a look at a story beat: `set_post_style("kuwahara", true)`, then `set_post_style_param("kuwahara", "blend", k)` every frame ([Scripting → Whole-frame styles](scripting.md#whole-frame-styles)). `examples/dream_story.scene.toml` falls asleep into Kuwahara and wakes again.

![The clearing falling asleep into Kuwahara at blend 0, about 0.25, 0.5, 0.75 and 1, the brush growing from radius 3 to 7 as it goes: the bench's legs and the barrel's hoops soften into strokes](../images/kuwahara-dream-fade.png)

*Frames from `flint-player examples/dream_story.scene.toml --frame-step 0.05 --screenshot-every 0.05` at 2, 3, 3.5, 4 and 5 s, cropped.*

- **How.** After a style whose blend is under 1 has drawn, the stage draws the style's input back over its output with the blend constant `1 - blend`. That is one full-screen draw, in the style's own GPU timing scope, and only while the blend is under 1. The mix is in the target's linear values (HDR at the HDR stage, linear light on an sRGB display target).
- **Cost.** No allocation per frame: one pipeline per target format, made the first time a blend is drawn, and the input's bind group is kept with the chain. A GPU test in `tests/stylize.rs` ramps Kuwahara's and invert's blends for 24 frames and counts the kept bind groups and GPU objects.
- **At 0 it's gone** (#399). A style whose table blend is 0 leaves its stage's chain: the frame is the one without it, byte for byte, and costs nothing. The style stays built and counts as on, so a fade can end at 0 and start up again later with no rebuild and no pop. Its mask and its claim on its entities' pixels stay too, so a whole-frame pencil still leaves those entities alone. The `ascii-whole-blend-zero` and `kuwahara-blend-zero` goldens compare with a render made without the style, and `ascii-whole-blend-half` pins the half-way fade:

  ```bash
  flint render examples/pbr_test.scene.toml --no-grid --ascii --style ascii:blend=0.5
  flint render examples/pbr_test.scene.toml --no-grid --ascii --sweep ascii.blend=0,0.25,0.5,0.75,1
  ```
- **Per-entity styles** keep their own `blend` on the component, which rides in the [style mask](#the-entity-style-mask) and mixes per pixel ([Blending in and out](#blending-in-and-out)). The table's `blend` fades the whole stage's result of that style, entities included.

### Revealing a whole-frame style

A blend fades the whole frame at once. A **reveal** (#406) shows the style only inside a region of the screen and leaves the picture it read outside, with a drawn line along the edge between them: the live world on one side of a ragged tear and the sketch on the other. Every style's table takes it, as every style takes `blend`:

```toml
[post_process.styles.pencil]
reveal = 0.5              # 0: none of the frame, 1: all of it
reveal_shape = "tear"     # tear, wipe, iris or panel
```

```bash
flint render examples/reveal_tear.scene.toml --style pencil:reveal=0.5
flint render examples/reveal_tear.scene.toml --sweep pencil.reveal=0.15,0.4,0.65 --sweep pencil.reveal_shape=tear,wipe,iris,panel
```

![The props clearing under pencil at reveal 0.15, 0.4 and 0.65 (across) for each shape (down): a tear ripping out from the middle with an inked, torn rim; a ragged wipe from the left; an iris; a comic panel the screen's shape](../images/reveal-shapes.png)

*`flint render tests/fixtures/scenes/stylize_props_plain.scene.toml --no-grid --style pencil:reveal=0.5 --sweep pencil.reveal=0.15,0.4,0.65 --sweep pencil.reveal_shape=tear,wipe,iris,panel --style-time 0.6`.*

| Key | Default | What it does |
|---|---|---|
| `reveal` | none | How far the region has grown, 0 to 1. With no `reveal` there is no region, whatever the other keys say. |
| `reveal_shape` | `tear` | `tear`: a ragged hole that rips open from a point. `wipe`: a ragged line that crosses the frame. `iris`: a circle from a point, its rim wobbling a little. `panel`: a straight rectangle the screen's shape, from a point out to every edge (the comic-panel cut). |
| `reveal_x`, `reveal_y` | 0.5, 0.5 | Where a tear, iris or panel starts, as a share of the frame (y down). |
| `reveal_angle` | 0 | A wipe's direction of travel in degrees: 0 from the left edge to the right, 90 from the top down. |
| `reveal_ragged` | by shape | How far the edge strays from the shape, as a share of the frame: tear 0.3, wipe 0.12, iris 0.05, panel 0. |
| `reveal_scale` | 2.5 | How many lobes the edge has across the frame's height. |
| `reveal_edge` | 2 | The line's width in points (a pen's, swelling and thinning along it); 0 draws none. |
| `reveal_ink` | `[0.12, 0.11, 0.1]` | The line's colour, `[r, g, b]` or `"#rrggbb"`, in display values at either stage. |
| `reveal_boil` | 12 | How many times a second of post time the edge's small teeth are redrawn, so it crawls like a line drawn again each frame; 0 holds it. The big lobes stay put. |
| `reveal_seed` | 0 | Another tear of the same shape. |

- **Tuned by eye in F4** (#875). Every style's section in **F4 → Styles** (and the player's F3 Styles panel) has a **Reveal** group under its own settings. Its checkbox switches the reveal on at `reveal = 1`, and switching it off takes every reveal key out of the style while keeping them, so the next time it goes on the tuned tear comes back. Under it are the amount, a shape combo, the ink's colour picker and a slider for each number above, with the table's ranges and help. A row the shape doesn't read is greyed out: Angle unless it's a wipe, Start x and y for a wipe. Dragging the amount tears the frame open live; **Copy TOML** and **Commit to scene** write the keys back, and a reveal switched off leaves the file when it is committed.

  ![F4 in the player over examples/reveal_tear.scene.toml: the amount dragged to 0.36 has torn the live clearing open into pencil through an inked, ragged edge. The Reveal group lists Amount, Shape, Ink and the eight numbers; Angle is greyed out for a tear, and the dot beside Amount marks it as differing from the scene](../images/f4-reveal.png)

  *`flint-player` with `--input-script` driving F4 on a copy of the scene without its director script, so no tween fights the slider.*
- **One call animates it.** `tween_post_style_param("pencil", "reveal", 1.0, 4.0)` eases the reveal from where it is (0 for a style that has none) to 1 ([Scripting → Whole-frame styles](scripting.md#whole-frame-styles)). `examples/reveal_tear.scene.toml` starts live, tears open into pencil over 4 s, and pulls the live frame back in with a wipe.
- **Deterministic.** The edge is the render-mode tears' noise (`tear_mask.wgsl`'s `fbm` and `value_noise`, with ridged teeth so it reads as torn paper, not a cut), on screen position and `reveal_boil` steps of post time. The same `--style-time` draws the same edge, byte for byte; the next step redraws its teeth.
- **The ends are exact.** At `reveal = 0` the style leaves its stage's chain, as at `blend = 0`, so the frame is the live one byte for byte and costs nothing. At 1 the reveal draws nothing, so the frame is the style's own. In between the amount grows the region about with the share of the frame it covers; a tear's last scraps of live picture linger in the corners until the end.
- **With `blend`.** Inside the region the style shows by its `blend`; outside, not at all. The ink is drawn at the blend too.
- **How and what it costs.** The stage draws it in place of the [blend](#blending-a-whole-frame-style): after the style, one full-screen draw puts what the style read back outside the region and lays the ink, premultiplied, in the style's own GPU timing scope. Each revealing style takes a 256-byte slot of one uniform buffer (16 a frame), so nothing is allocated per frame. Measured with `--gpu-time 200` at 1080p on an RTX 4070: pencil's pass went from 0.477 to 0.568 ms p50 with `reveal = 0.5`, about 0.09 ms for the reveal, against 0.024 ms for a plain `blend = 0.5`.
- **Goldens.** `pencil-reveal-tear-half` pins the half-torn frame at post time 0.6, `pencil-reveal-panel` a panel, and `pencil-reveal-zero` compares reveal 0 with the frame without the style.

### Style blends and render-mode tears

`--render-mode` 1--4 (matrix, blood, drunk, tron) and a style blend can both look like reality tearing into text, but they work at different levels (and a blend can [follow the tear](#a-blend-that-follows-the-tear)):

| | Render-mode tears | Style blend |
|---|---|---|
| Where | Composite pass, before display styles | Display-stage style, after the composite |
| What it covers | The whole screen, through `tear_mask`: drifting fbm patches or an iris from the centre, in screen space | One entity and its subtree, through the style mask, on the entity's own cells |
| Driven by | `mode_mix` and `mode_params` (a scene-wide value) | `blend` on each entity's component |
| The look | Fixed per mode (mode 1's glyphs are an 8x12 dot-matrix built into `composite_shader.wgsl`) | Any style's settings: fonts, ramps, palettes, edges, grids |

Neither duplicates the other's job. Tears change how the *world* is seen, and a blend changes how one *thing* is drawn. They compose: an ASCII prop inside a matrix tear is glyphized from the tear's picture, since display styles read the composite's output. Mode 1's glyphs and the ASCII style do overlap as looks, and the `matrix` look (`--look matrix`) is the ASCII style's take on it.

### A blend that follows the tear

`blend_source = "tear"` (#397) makes an entity's blend follow the tear: at each pixel it is the entity's `blend` times the tear's mask there, so the prop turns to text only where reality tears and stays its own shading everywhere else. It goes on the style component or a `style_mask`, like `blend`, and is the root's, inherited by its subtree:

```toml
[entities.barrel.ascii_style]
blend_source = "tear"     # "own", the default, is the blend alone
transition = "dissolve"   # cells turn whole at the tear's frayed rim
```

Render mode 6 is a tear with no look of its own: the composite draws nothing for it, so the props that follow it are the only thing it shows. Modes 1--4 tear too, and then the props turn inside the mode's own holes (an ASCII prop in a matrix tear is glyphized from the code rain, as above). A script drives it as any mode, every frame it shows, and a scene can't set a render mode:

```rust
fn on_init() {
    set_render_mode_params(2.4, 0.0, 6.0, 0.0);   // patch scale 2.4, fbm patches
}

fn on_update() {
    set_render_mode(6, 0.5);                      // half the clearing torn
}
```

```bash
flint render examples/ascii_tear.scene.toml --render-mode 6 --mode-mix 0.5 --mode-params 2.4,0,6,0
```

![The clearing at tear mix 0, 0.3 and 0.55 under render mode 6, and at 0.3 under mode 1: the props turn to green glyphs only inside the tear's patches, which mode 1 shows as code rain, while the grass and ground stay as they were](../images/ascii-tear.png)

*`flint render examples/ascii_tear.scene.toml --render-mode <m> --mode-mix <x> --mode-params 2.4,0,6,0 --no-grid`, cropped.*

- **One mask, two readers.** The tear is `tear_mask_at` in `crates/flint-render/src/tear_mask.wgsl`, which Rust prepends to both the composite and the style mask's shader. The mask pass multiplies it into the blend it writes into the mask's `.g`, so every reader of the blend gets it: ASCII's fade and dissolve, and pencil, watercolor and user styles through the framework's [cross-fade](#blend-for-free). Its uv is the mask pixel's centre over the mask's size, the composite's own at any render scale, so a prop turns exactly where the composite tears.
- **No tear, no cost.** While the frame draws no tear (`render_mode` not 1--4 or 6, `mode_mix` at most 0.001, or post-processing off), a torn entity counts as blend 0: its id is faded, so with nothing else styled neither the style's pass nor the mask is drawn and the frame is the unstyled one, byte for byte ([At blend 0](#blending-in-and-out)). The renderer checks the config each frame, since a script sets the mode after the world is read. The golden `ascii-tear-calm` holds it.
- **Settings.** `mode_params` are the tear's ([Render modes](post-processing.md#render-modes)): `x` the patch scale (0 = 3), `y` fbm patches (0) or an iris from the centre (1). The uniform each masked draw has grew from 16 to 48 bytes to carry them, in its existing 256-byte slot.
- Scripts set it with `set_style_param(id, "blend_source", "tear")` and read it back with `get_style_param`; it is a choice, so it doesn't ease.

`examples/ascii_tear.scene.toml` is a 12-second loop: the tear opens over the clearing, nearly swallows it, and closes. The goldens `ascii-tear` and `ascii-tear-matrix` pin it under modes 6 and 1 on `tests/fixtures/scenes/stylize_props_tear.scene.toml`.

A screen field a game fills itself (a shockwave ring, a flashlight cone, the wooze field) as a blend source is not built yet (#859).

## Palette

The `palette` style (#324) draws the finished picture in a palette's colours and nothing else: no glyphs, no strokes. It is the [palettes](#colour-modes) on their own, as a look: a CGA screen, a green phosphor tube, ink on paper, or a game's house palette.

```toml
[post_process.styles.palette]
palette = "phosphor"   # a built-in, palettes/<name>.palette.toml beside the scene, or a path
dither = 1.0           # 0 = flat bands; 1 = an ordered 4x4 dither between colours
pixel = 4              # block size in points: 1 = per pixel, more = a chunky low-res screen
```

```bash
flint render examples/mesh_meadow.scene.toml --output meadow.png \
  --style palette:palette=phosphor,dither=1,pixel=4
```

| Setting | Default | Meaning |
|---|---|---|
| `palette` | `cga` | Any palette ASCII takes: `cga`, `cga-warm`, `cga16`, `phosphor`, `amber`, `mono`, `paper`, or a file. One that can't be found warns once and uses `cga` |
| `dither` | 0 | For `nearest` palettes: a colour lying `s` of the way from its nearest colour to the next takes the next in `s x dither` of the blocks, by a 4x4 Bayer pattern. For `luminance` ladders: the tone moves up to half a rung either way. The pattern is fixed to the blocks, so a still camera gives a still picture |
| `pixel` | 1 | Block size in points (times the display's pixels per point). A block takes the mean of its pixels, in linear light, and is one colour |
| `chroma` | 1 | For `nearest` palettes: scale each block's saturation (OKLab a/b) before matching, as ASCII's [`chroma`](#palettes-match-in-oklab) does (0--4) |

**The ground is a colour here.** ASCII never inks a glyph in the palette's background, since the glyph's density already says how bright the cell is. The palette style has nothing else to say it with, so the ground joins the inks: `nearest` palettes list it first, and a `luminance` ladder takes it at its own lightness, so phosphor's near-black is the bottom rung and paper's cream the top. `Palette::table()` gives that list and `Palette::quantize()` the colour it draws for a pixel.

**One matcher.** A `nearest` palette picks with `pal_pick` in `crates/flint-render/src/palette.wgsl` (`palette::PALETTE_WGSL`, OKLab distance since #320) in both styles. Each shader concatenates the same file and defines `palette_ink(i)` and `palette_ink_lab(i)` over its own uniform, so a palette can't mean one thing in ASCII and another here. `Palette::pick` (and `palette::pick_in` over any list) is the same function on the CPU, and `the_palette_matcher_agrees_with_its_cpu_reference` holds them to each other. A `luminance` ladder is climbed by each style itself, since ASCII climbs it by the cell's tone; `the_palette_style_quantizes_the_frame_to_its_table` holds the palette style's ladder and nearest picks to `Palette::quantize`, flat and dithered. A new style that reads a palette should include the file rather than copy it.

`examples/pbr_test.scene.toml` and `examples/mesh_meadow.scene.toml` at 1280x720, cropped at 1:1; the right column is `dither=1,pixel=4`:

| | flat (`pixel=1`) | dithered, 4-point blocks |
|---|---|---|
| `cga` | ![cga](../images/palette-cga-pbr-test.png) | ![cga dithered](../images/palette-cga-dither-pbr-test.png) |
| | ![cga](../images/palette-cga-mesh-meadow.png) | ![cga dithered](../images/palette-cga-dither-mesh-meadow.png) |
| `phosphor` | ![phosphor](../images/palette-phosphor-pbr-test.png) | ![phosphor dithered](../images/palette-phosphor-dither-pbr-test.png) |
| | ![phosphor](../images/palette-phosphor-mesh-meadow.png) | ![phosphor dithered](../images/palette-phosphor-dither-mesh-meadow.png) |
| `paper` | ![paper](../images/palette-paper-pbr-test.png) | ![paper dithered](../images/palette-paper-dither-pbr-test.png) |
| | ![paper](../images/palette-paper-mesh-meadow.png) | ![paper dithered](../images/palette-paper-dither-mesh-meadow.png) |
| `cga16` | | ![cga16 dithered](../images/palette-cga16-dither-pbr-test.png) ![cga16 dithered](../images/palette-cga16-dither-mesh-meadow.png) |

Undithered `cga` is a hard look on a naturalistic scene: four colours, two of them black and white. Matching in OKLab, pbr_test's blue-grey floor (#636380) takes magenta, its nearest hue, and the meadow's dark grass goes to black under a white sky. That is the matcher being honest, not a bug, and the dither is what CGA artists used for it: the floor comes out a cyan-and-magenta weave. Ladders (`phosphor`, `amber`, `mono`, `paper`) are kinder, since every tone has a rung.

On an RTX 4070 at 1080p the pass costs 0.033 ms with `phosphor` (a ladder picks without a search), 0.086 ms matching `cga16`'s 16 colours, and 0.108 ms for `cga16` dithered in 4-point blocks (four bilinear taps a pixel; reading all 16 texels was 0.31 ms).

Palette runs after watercolor, ASCII and pencil, so with them on it quantizes their output, glyphs and strokes included, and before display bloom, so the bloom glows in the palette's colours instead of being quantized away.

## Watercolor

The `watercolor` style (#272) paints the picture as transparent pigment on paper. Colour bleeds into soft washes, the washes are laid in a few glazes, their borders wander, pigment pools darker along edges and settles into the paper's grain:

```toml
[post_process.styles.watercolor]
wash = 4.0          # how far pigment diffuses, in points (the wash's blur radius)
bleed = 0.55        # how much of the colour is the diffused wash
layers = 4          # glazes: tone steps, each edged darker on its darker side (0 = smooth)
wobble = 2.0        # how far borders and silhouettes wander, in points
wobble_scale = 40   # the wander's wavelength, in points
edge = 1.0          # pigment pooling at colour edges and glaze borders
edge_depth = 0.8    # pooling on the near side of silhouettes, from depth
edge_width = 1.5    # how wide that rim is, in points
turbulence = 0.35   # low-frequency blotches of thicker and thinner pigment
granulation = 0.5   # pigment settling into the paper's valleys, strongest in dark washes
grain = 0.35        # the paper's relief, shading even bare paper
grain_scale = 1.0   # the paper's tooth (bigger = coarser)
pigment = 1.0       # overall density: below 1 dilutes, above 1 deepens
paper_r = 0.97      # the paper's colour (paper_g 0.94, paper_b 0.87)
paper_image = "none"  # or a papers/<name>.png beside the scene (#390)
scope = "screen"    # or "entities": a preset for watercolor_style entities
```

![examples/mesh_meadow.scene.toml under --style watercolor: the grass laid in glazes with pooled dark rims, the path a granular wash, the sky bare cold-press paper](../images/watercolor-meadow.png)

*`flint render examples/mesh_meadow.scene.toml --style watercolor`, 1280x720.*

![examples/watercolor.scene.toml: only the YBot, the two barrels and the crate are painted; the meadow and the ground stay as rendered, and the tag on the crate stays unpainted](../images/watercolor-entities.png)

*`flint render examples/watercolor.scene.toml --no-grid`, 1280x720: per entity, as authored.*

![stylize_props with Kuwahara at radius 4 under watercolor at three glazes; the props' style_mask "ascii" glyphs and "pencil" hatching draw over the paint](../images/watercolor-kuwahara.png)

*`--style kuwahara:radius=4 --style watercolor:layers=3`: the two chain.*

![The watercolor look's layers swept 0, 3, 5, 8 on stylize_props](../images/watercolor-sweep-layers.png)

It runs at the **display stage**, after tonemapping, because it paints *on* paper: the paper and its grain must not be fogged, bloomed or graded as if they were in the world. Three passes: a separable Gaussian makes the wash (two targets of the style's own), then the paint pass wobbles where it reads the picture, the wash and the mask, mixes the wash in, steps it into glazes, works out the pigment's density, and lays it on the paper.

- **Density.** The colour is treated as the pigment layer's transmittance over white paper, and a layer `d` times as thick transmits `C^d` (Beer–Lambert). White stays the paper and black stays black; a mid-tone moves most. Edges, glaze borders, depth rims, turbulence and granulation all add to `d`.
- **Paper** lives in screen space, in points, so its grain holds still as the camera moves and is the same size to the eye at any `--pixels-per-point`. The noise is integer-hashed, so a frame is the same on every run.
- **A paper image** (#390): `paper_image = "cold_press"` paints on `papers/cold_press.png` (found as for [pencil](#paper-and-strokes-from-files)), one texel every `grain_scale` points. Its height, measured against the image's own mean and spread, replaces the noise, so `grain` (the relief) and `granulation` (pigment settling in the valleys) read the scan. Its local hue variations tint `paper`. One paper serves a frame: the scene's, else the first entity's.

![mesh_meadow under watercolor at 1:1, procedural paper and cold_press: the path's wash settles into the scan's nodules](../images/watercolor-paper-image.png)
- **Per entity.** A `watercolor_style` component (its fields are the keys above, `paper = [r, g, b]` for the paper's colour) or a `style_mask { style = "watercolor" }` paints an entity with no table, the rest of the frame passing through, exactly as [per-entity ASCII](#per-entity-ascii) works: the scene's table (with `scope = "entities"`) is the preset, entities with equal settings share an id, `enabled = false` opts out, descendants inherit. The mask is read at the wobbled position, so a painted prop's silhouette wanders a little over its surroundings, as a real wash's would. One frame holds 15 variants. `examples/watercolor.scene.toml` paints a static mesh, a skinned one, two props with settings of their own and an inherited one, and leaves one out.
- **One wash radius a frame.** `wash` is the largest any variant (or the scene) asks for; every other key is per entity.
- **With Kuwahara** they chain: Kuwahara (HDR stage) flattens the scene into brush pools, and watercolor then paints those pools on paper (`--style kuwahara:radius=4 --style watercolor:layers=3`). They share nothing, and neither needs the other.
- **With ASCII and pencil**, watercolor runs first by default (registration order), so glyphs and hatching draw over the paint. Give it a higher `order` to paint over them instead ([Order within a stage](#order-within-a-stage)).
- The `watercolor` look (`--look watercolor`) is the style with softened light (no bloom, lighter SSAO), `pigment = 0.85` and four glazes. `ink_wash` is still the Kuwahara-built monochrome look.

```bash
flint render examples/mesh_meadow.scene.toml --style watercolor -o meadow.png
flint render examples/watercolor.scene.toml -o props.png                       # per entity, as authored
flint render examples/stylize_props.scene.toml --look watercolor --sweep watercolor.layers=0,3,5,8 -o glazes.png
```

At 1080p on an RTX 4070 the whole-frame pass costs about 0.43 ms (`mesh_meadow`, `--gpu-time 240`). The per-entity demo costs 0.32 ms plus 0.03 ms for the mask: the wash is still blurred over the whole frame, and only the paint pass skips unpainted pixels. `--style watercolor:off` turns the style off everywhere, the entities that carry a `watercolor_style` of their own included (#376), so the props render as the scene does underneath, with bloom, SSAO and the rest kept. In a scene, a table's `enabled = false` is only the table's: an entity's own component still switches the style on for its pixels, so set `enabled = false` on the entities, as with ASCII.

## Display bloom

Display styles draw after the scene's bloom has run, so their bright marks can't feed it. ASCII's `glow` makes up for that inside each glyph's cell, but it stops at the cell's edge: a glyph never lights its neighbours, and a bright run of cells doesn't bloom as one. The `display_bloom` style (#321) is a bloom over the displayed picture, for any display style that wants one (ASCII's glyphs, pencil's highlights, a CRT look):

```toml
[post_process.styles.display_bloom]
intensity = 1.5    # how much halo is added
threshold = 0.55   # display value (0-1) a pixel's brightest channel must pass
knee = 0.25        # softness of that threshold, as a share of it
levels = 5         # how far it reaches: each level doubles it (in points)
saturation = 1.0   # 1 keeps the glyph's hue, 0 glows white
```

![pbr_test in phosphor ASCII at 3x, glow 0.6 alone on the left and with display bloom on the right: the bright box's glyphs share one halo that spills across their cells and over the grid lines beside it](../images/display-bloom-phosphor-crop.png)

*A crop of `flint render examples/pbr_test.scene.toml --ascii --ascii-palette phosphor --ascii-glow 0.6` at 1280x720, shown at 3x, without and with `--style display_bloom`.*

- **Where it runs.** It's registered after ASCII and pencil, so by default it blooms what they drew. Give it an `order` to place it elsewhere in the display chain ([Order within a stage](#order-within-a-stage)).
- **The threshold is on what you see.** A pixel passes by its brightest channel in display values (0--1), with a soft knee. The halo itself adds in linear light. At the defaults the dim grass glyphs stay clean and only the lit box and the ridge glow; a lower `threshold` or a wider `knee` lifts the whole field.
- **The chain** has the HDR bloom's shape: a threshold into a half-size mip (four taps over each 4x4 block, so a one-pixel stroke lands whole), a 13-tap downsample per level, a tent upsample added back up level by level, and a composite that adds the result over the input. The mips are `Rgba16Float` textures of the style's own (`StyleFrame::target_sized`), the first mip chain a display style keeps. Each step has its own uniform slot, because every write lands before the frame's one submission.
- **`levels` is in points.** On a 2x display the chain takes one more level, so the halo keeps its size next to cells that also grew. The intensity is divided by the levels drawn, so changing the reach doesn't change the brightness.
- It's a whole-frame style with no per-entity component.

At 1080p on an RTX 4070 it costs **0.066 ms** (5 levels; `flint render examples/pbr_test.scene.toml --ascii --ascii-palette phosphor --style display_bloom --width 1920 --height 1080 --gpu-time 240`). Its mips add about 5.5 MB at 1080p (a half-size `Rgba16Float` texture and the quarter, eighth... ones under it). The `display-bloom` golden pins it over phosphor ASCII.

```bash
flint render examples/pbr_test.scene.toml --ascii --ascii-palette phosphor --style display_bloom -o glow.png
flint render examples/pbr_test.scene.toml --ascii --ascii-palette phosphor --style display_bloom --sweep display_bloom.knee=0,0.25,0.5 -o knee.png
```

## Looks and sweeps

A **look** (#279) is a named preset you can put over any scene. It lives in `styles/<name>.look.toml`, next to the `*.styleguide.toml` generation guides, and has two tables. `[look]` says what the look is and which sweeps to try first. `[post_process]` is a block in exactly the scene format. The look's block is merged into the scene's own `[post_process]` key by key: tables merge and everything else replaces, so a look changes only the keys it names. A field or style the engine doesn't know is an error that names the closest real one, because otherwise a typo would quietly fall back to a default.

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

Nine looks ship, compiled into the engine (`flint_render::look`) so they work from a game repo and on the web as well. A look is found by [the one asset search](assets.md#where-the-engine-looks-for-a-file) (kind `looks`): `styles/<name>.look.toml` beside the scene, in each directory above it up to the project root, then in the mounted engine (`<root>/engine/styles/`, where a game's engine subtree keeps these nine as files). A file found there takes the place of a built-in with the same name; the built-in is what is used when there is no file. `--look` also takes a path ending `.look.toml` (or `.toml`), tried from the scene's folder and each folder above it. The working directory's `styles/` is not searched: run from a project's root it is the root's, which the search reaches anyway. `flint asset where looks graphite --scene <scene>` shows which it is.

| Look | Built from |
|------|------------|
| `ascii_noir` | mono ASCII, black ground, hard contrast, heavy vignette |
| `matrix` | phosphor-palette ASCII with glow, `detailed` ramp |
| `ink_wash` | Kuwahara radius 6, desaturation, warm paper fog and grade, grain: monochrome brush pools |
| `watercolor` | the watercolor style in four glazes, pigment 0.85, no bloom, light SSAO |
| `graphite` | the pencil style at a 5-point spacing, contrast 1.4, doubled outlines (inverted mono ASCII before #273) |
| `coloured_pencil` | the pencil style at `color = 0.6`: the scene's colours in a warm lead on cream paper, two layers, heavy grain (#391) |
| `blueprint` | the pencil style in white lead on blue paper, `direction = "screen"`, no wobble, sketch or texture lines: a technical drawing (#391) |
| `charcoal` | the pencil style at an 11-point spacing and 3-point strokes, rubbed shade 0.8, heavy grain, doubled outlines (#391) |
| `rotoscope` | the pencil style ink-forward on a blank white sheet: contours at 1.5 points, one layer of screen hatching only in the darks (brightness 0.5), `grazing = 1` so the floor is one horizon line, boil 12 a second; no bloom, SSAO or vignette (#404) |

![The four pencil looks on stylize_props: graphite, coloured_pencil, blueprint, charcoal](../images/look-sweep-pencil.png)

```bash
flint render --look help                                   # every look, its tune sweeps and notes
flint render scene.toml --look ink_wash -o wash.png
flint render scene.toml --look ink_wash --style kuwahara:radius=3 -o sharper.png   # flags apply over the look
```

### Looks in scenes, F4 and scripts

A scene can name a look in its own block (#356). The look goes **under** the scene's keys, so the scene takes the look and keeps whatever it sets itself:

```toml
[post_process]
look = "graphite"       # a styles/<name>.look.toml the scene can see, or a built-in
exposure = 1.25         # the scene's key wins over graphite's 1.15
vignette_enabled = true

[post_process.styles.pencil]
spacing = 9             # the rest of graphite's pencil table stays
```

`--look` goes the other way, over the scene's keys, so a look can be tried on any scene as it stands. A scene whose block is only `look = "graphite"` draws exactly what `--look graphite` draws on the same scene without a block: the two renders of `examples/looks.scene.toml` are byte-identical. `flint render` prints `Scene look: graphite`, and `flint validate` warns about a look it can't find or one whose keys don't fit. In the player a missing look is a warning and the scene draws without it.

The player and the viewer (`flint edit scene.toml`) read the look when the scene loads. **F4 → Look** lists every look the scene can reach (each `styles/` from its folder up to the project root, the mounted engine's, then the built-ins no file shadows) and switches to the one you pick. The pick replaces the whole post config, so edits made in F4's other sections before the pick are gone. The viewer keeps the pick across hot reloads, so you can edit a look file in the scene's `styles/` and watch it change. Scripts switch looks with `set_look("matrix")` and take them off with `clear_look()` ([Scripting → Style API](scripting.md#style-api)). Every switch starts again from the scene's own keys, so nothing from the previous look carries over.

![F4's Look picker in the player: examples/looks.scene.toml's graphite swapped for watercolor by hand; the post chain below mirrors the new exposure and vignette](../images/f4-look-picker.png)

![examples/looks.scene.toml in flint-player: scripts/looks_tour.rhai calls set_look every three seconds of game time (graphite, ascii_noir, matrix, ink_wash, watercolor) and then clear_look](../images/looks-tour-player.png)

*Captured with `flint-player examples/looks.scene.toml --frame-step 0.1 --screenshot tour.png --screenshot-at 1.5 --screenshot-every 3`.*

**`--sweep KEY=V1,V2,...`** renders the same frame once per value and lays the frames out as one labelled contact sheet, four across, in the ASCII style's built-in glyphs. KEY can be:

- any `--style` key, written `<style>.<key>`: `ascii.cell=6,8,12`, `ascii.ramp=classic,blocks`, `kuwahara.radius=2,4,6,8`. Values are checked against the style's range;
- any `[post_process]` field, written `post.<field>`: `post.exposure=0.8,1.2`. Use `|` between values to sweep a vector: `post.fog_color=0,0,0|0.7,0.75,0.82`;
- `look=ascii_noir,matrix,ink_wash,graphite`, to compare whole looks;
- a Living Props wooze knob, written `wooze.<knob>`: `wooze.glide=0,0.25,0.5,1`, `wooze.preset=boil,morph` (knobs `amplitude`, `step`, `glide`, `jitter`, `preset`). Each such column is a run of its own from time 0 (see [Sweeping a Knob](../guides/headless-rendering.md#sweeping-a-knob)).

Each family is one key prefix on the one flag, parsed by one type (`commands/sweep.rs`, #353), and every family lands on the same sheet. Each cell is 640x360 unless you pass `--width` and `--height`. The scene loads once and every column re-renders the same frame, so four columns of `stylize_props` take about 13 s in a debug build. A sweep goes over `--look` and every other flag. When two neighbouring columns come out pixel-identical, `flint render` warns that the sweep showed nothing between them: the style was off, nothing on screen read the key, or both values clamped to one.

The ASCII looks set no `edges` key, so they draw the [edge glyphs](#edges) by default; add `edges = 0` to a look's `[post_process.styles.ascii]` for tone alone. The two sweeps below were captured before edges landed, so the props in them aren't outlined.

![The four looks on stylize_props, one sweep](../images/look-sweep-stylize-props.png)

![ascii_noir's brightness swept: at 0 the ground is black, at 0.15 it comes back, at 0.45 the props drown](../images/look-sweep-noir-brightness.png)

### Two keys as a grid

Some keys only make sense together. Brightness and contrast both move the ground, so a contrast that looks right at one brightness can black the scene out at another. **Give `--sweep` twice** and the sheet becomes a grid (#358): the first key's values run across the columns, the second key's run down the rows, and every cell takes one value from each. A grid never wraps, so a column holds one value all the way down and you can read along either key. A grid takes up to 48 cells. You can't sweep the same key twice, or two `look=` lists; one axis can be a look and the other a post field or style key.

```bash
flint render examples/stylize_props.scene.toml --look ascii_noir \
    --sweep ascii.brightness=0,0.15,0.3,0.45 --sweep ascii.contrast=1,1.6,2.2 -o noir-grid.png
```

![ascii_noir's brightness across and contrast down: contrast 2.2 at brightness 0 or 0.15 leaves only the lit pool, contrast 1 brings the ground back at every brightness](../images/sweep-grid-noir-brightness-contrast.png)

The pixel-identical warning works along both axes, and in a grid it says how many rows (or columns) the pair matched in: `ascii.cell=6 and 8 are pixel-identical: ... (in 1 of 3)`.

### Cells at the final scale

A style sized in points, such as ASCII's `cell` or pencil's `spacing`, draws as many glyphs across a frame as the frame has points. A 640-wide cell at `cell = 8` is 80 glyphs across, while the 1920-wide final render is 240, so a sheet of small frames can pick a winner that the final render disagrees with. Two flags draw each cell at the final size instead (#358):

- **`--sweep-crop X,Y,W,H`** draws every cell at `--width`x`--height` (1920x1080 unless you set them) and keeps that region pixel for pixel. The glyphs come out exactly the size the final render draws them, but you only see part of the frame. Choose a region that holds what the key affects, such as a prop's edge for glow or a gradient for a ramp.
- **`--sweep-scale full`** draws every cell at the full size and shrinks it to 640 wide. The composition matches the final render (where glyphs land, how much of the frame the tone fills), but the glyphs blur.

Both flags work with a grid. The sheet's subtitle states the scale, for example `1:1 640x360 at 760,520 of 1920x1080` or `1920x1080 shown at 33%`, so a reader knows what they are looking at. A crop that goes past the frame is refused before anything is drawn. `--sweep-scale cell` is the default.

```bash
flint render examples/stylize_props.scene.toml --look ascii_noir \
    --sweep ascii.glow=0,0.6,1.2 --sweep-crop 760,520,640,360 -o glow.png
```

![ascii_noir's glow swept as 1:1 crops of a 1080p frame: the radio and the barrel at their final glyph size](../images/sweep-crop-noir-glow.png)

Rendering at full size costs more: each cell is a whole 1080p frame. The pixel-identical check compares the full frames before they're cropped, so a crop can't hide a difference elsewhere in the frame.

### Measuring and refining a look

A sheet is judged by eye. **`--report FILE.json`** (#357) also measures each column, from the same pixels the sheet is made of: mean luminance, the share of near-black (luma ≤ 24) and near-white (≥ 232) pixels, edge density (the share of pixels whose right + down luma step passes 24), and, when the ASCII style drew, the grid's size, its share of blank cells and the share of cells on each step of the ramp, darkest first. Edge glyphs are counted in `off_ramp`. The same sweep prints a line per column:

```text
$ flint render examples/stylize_props.scene.toml --look ascii_noir \
    --sweep ascii.brightness=0,0.15,0.3,0.45 --report brightness.json -o brightness.png
  [0] luma 0.10  black 82%  white 1%  edges 24%  blank cells 29%
  [0.15] luma 0.18  black 69%  white 2%  edges 42%  blank cells 5%
  ...
```

**`--reference ref.png`** adds each column's distance to a target image, using the metric `flint gen --ai-refine` uses (#183): mean colour, a 16-bin luminance histogram and a 16x16 luminance layout, as a score out of 100 where 0 is the reference itself. It's a ruler, not a judge. It can say whether one column is closer than another, but it doesn't measure taste. On a sheet each label gets `d <score>`, the nearest column is starred, and `nearest` in the report names it. Under `--sweep-crop` or `--sweep-scale full` the image measures and distance are taken from the cell as the sheet shows it, while the ASCII section still counts the whole drawn frame's grid. A two-key grid reports every cell, each `value` naming both keys, but its labels carry no distances.

**`--ai-refine`** closes the loop: `flint render scene --look <x> --reference ref.png --ai-refine`. Each round an agent sees the reference, the current render, the look's block, every key it may change (with ranges) and every edit tried so far with its score. It proposes up to `--refine-candidates` (4) candidates in `--sweep` key syntax. Each candidate is checked like a sweep value, then rendered and measured. The best is kept if it lowers the distance by more than 0.05. The loop stops after `--refine-rounds` (4), or after two rounds in a row with no gain, or when the agent has nothing left to propose. It writes:

- `<look>_refined.look.toml` beside `-o` (or `--refine-out`): the kept block, plus a note of what was kept and what it cost;
- the same path with `.json`: the log of every candidate, its score and image measures, and whether it was kept;
- `-o`: a sheet showing the reference, the start, and each kept round.

`--ai-provider anthropic` (the default) asks Claude (`ANTHROPIC_API_KEY`, `--ai-model`). Every call goes through the procgen `CostMeter`: it's priced, appended to `.flint/procgen_costs.json`, and refused before sending when it could pass `--max-cost`. `--ai-provider mock` is free and offline. It walks the look's own `tune` sweeps one untried value at a time, which turns the loop into a measured coordinate search. Toward a hidden target (ascii_noir at cell 12, brightness 0.3, contrast 1.2), the mock went 7.68 → 0.00 in five rounds. Contrast 1.2 scored *worse* than the start on its own, and only helped once the other two had moved.

Toward the `matrix` look's render, with Claude (four calls, $0.115), ascii_noir went 25.66 → 15.09. Claude turned the colour to `tint` and the ramp to `detailed`, then found that `glow` was the strongest lever. It also learned from the log that whitening the tint and turning off the vignette both scored worse:

![Refining ascii_noir toward the matrix look: reference, start 25.7, then rounds at 25.0, 19.2, 15.2, 15.1](../images/look-refine-noir-to-matrix.png)

### Choosing cell size and ramp

- **The cell is in points, not image pixels.** A 640-wide sweep cell at `cell = 8` is 80 glyphs across, while the same look at 1920 is 240. Glow and fine ramps in particular read differently at full size. Add `--sweep-crop` or `--sweep-scale full` (see [Cells at the final scale](#cells-at-the-final-scale)) to sweep at the final size, or confirm the winner with one render at that size.
- **Size by columns.** About 200–240 columns (cell 8–9 at 1080p) keeps props readable. Below about 120 columns (cell 16 and up at 1080p) the image becomes a poster: silhouettes only, no surface detail. Cells of 4–5 read as texture or hatching, not as letters.
- **Pick a ramp for the tones the scene has.** `classic` (` .:-=+*#%@`, 10 steps) is the general-purpose ramp. `detailed` (70 glyphs sorted by coverage) gives the smoothest gradients and the most letter-like texture, so it suits small cells. `minimal` (` .oO@`) and `blocks` (` ░▒▓█`) have five steps each: they band hard, which suits big cells and posters.
- **Brightness first, then contrast.** When a scene comes out mostly black or mostly blank, sweep `brightness` before `contrast`. On `stylize_props` with `ascii_noir` at brightness 0, contrast 1.6 and 2.2 left the ground black. Contrast 0.6 or 1 brought the ground back, but it also flattened the props into it. Brightness 0.15 at contrast 1.6 brought the ground back and kept the props dark against it.

## Where styles run

Each style belongs to one of two stages:

```text
scene (HDR, render scale)
  -> style mask (Rg16Uint: id + blend, only while a style reads it)
  -> HDR styles (Kuwahara)          linear Rgba16Float, scene resolution
  -> SSAO, volumetric (depth), bloom (the original HDR, or the HDR styles' result: feeds_bloom)
  -> composite: fog, DoF, tonemap, grade, vignette, grain, render modes
  -> Display styles (watercolor, ASCII, pencil, palette, display bloom, invert) surface format, output resolution
  -> FXAA -> surface -> HUD
```

- **HDR stage.** Linear light at the scene's render scale. Everything after it shades the style: fog fades it, bloom haloes its bright strokes, the grade tints it. Bloom reads the unstylized scene unless a style feeds it (below). SSAO and volumetrics read depth and shadows, never colour, so no style changes them; the composite lays them over the stylized image.
- **Display stage.** The finished, tonemapped picture at output resolution, after the render-scale upscale and before FXAA and the HUD. This is where glyphs and lines stay crisp under `--render-scale` (the ASCII spike's finding).

Styles of one stage run one after another, each reading the one before it. The framework ping-pongs between two targets it owns, so any number of styles chain without a style knowing its neighbours.

The display stage keeps its memory to two full-size textures (#302). At 1440p each one is about 15 MB, which matters on phones and the web. The composite draws into the stage's input, and once the first style has read that input it is free to be a ping. With FXAA on, FXAA's intermediate is the second texture: the composite draws into whichever of the two leaves the last style writing the intermediate. So one style needs one texture of its own (two with FXAA's), and longer chains need two (still two with FXAA's). The output is byte-identical to the old chain. A style's own intermediates come on top of these; watercolor's two washes are full size, for example. The player's F3 stats show every target as `Style targets: N (MB, K full-size)`, where the full-size count includes FXAA's intermediate. The count is everything asked for since the last resize, so switching a style off doesn't lower it.

### Order within a stage

By default a stage runs its styles in registration order: kuwahara, watercolor, ascii, pencil, palette, display_bloom, invert, mask_view, edges_view, then user styles in the order they loaded. A style's table can give it a place of its own with `order` (#299). Lower runs first; a style without one stands at its registration index (watercolor 1, ascii 2, pencil 3, palette 4, display_bloom 5, invert 6), and equal orders keep registration order:

```toml
# ASCII first, then watercolor paints over the glyphs
[post_process.styles.ascii]
[post_process.styles.watercolor]
order = 9
```

![The props scene in both orders: watercolor then ASCII (the default), and ASCII then watercolor](../images/stylize-order.png)

- Any number works: an integer, a fraction to slip between two (`order = 2.5`), or a negative to go before everything.
- Only styles of the same stage compare. An HDR style always runs before a display style.
- A table switches its style on, so to place a style without turning it on write `enabled = false` beside `order`. Kuwahara takes `order` in its `[post_process.styles.kuwahara]` table like any other style.
- `--style ascii:order=0` sets it from the command line, and `--sweep watercolor.order=0,9` lays the orders side by side.
- The F4 Styles section opens with each stage's chain, first to last; **swap** runs a style before the one ahead of it, and Copy TOML writes the `order` back out.

`order` is a key of every style's own table, not a `style_order = [...]` list on `[post_process]`, because everything else about a style lives in its table: a [look](#looks-and-sweeps) or a `--style` word that adds one style can place it without restating every other style, Copy TOML stays one block, and a list naming a style the scene doesn't use would need its own warnings. The cost is that the whole order isn't written in one place, which is what the F4 chain line shows instead. No style may name a parameter or option `order` or `enabled`.

### Each pixel is one style's

An entity that asks for a style owns its pixels for that style (#384). With ASCII on for the whole frame, a cask carrying `style_mask { style = "pencil" }` stays a pencil sketch: ASCII leaves it, and pencil draws it from the scene as rendered, not from ASCII's glyphs. The rule runs both ways, and in either chain order: an ASCII barrel in a pencil frame stays glyphs.

```sh
flint render examples/stylize_props.scene.toml --style ascii --no-grid --output props.png
```

![Whole-frame ASCII over stylize_props, before and after: the pencil cask and crate were glyphs sketched over, now they are sketched](../images/stylize-ownership.png)

The framework does it, so a new style (a user style included) gets it without code of its own. After each style draws, an ownership pass puts the style's input back on every pixel whose mask id another running style owns. The owner of an id is the style its variant names, when that style is registered and on this frame. An opt-out (`enabled = false`) is nobody's, so the whole-frame style draws it: the tag on the crate is glyphs. The pass reads the mask's blend, so a foreign entity fading out of its style (`blend` toward 0) fades into the whole frame's style. Styles read the same list as `StyleFrame::foreign_ids()`: pencil keeps its contours and hatching off those pixels, and ASCII's text grid (`--ascii-text`) writes their cells as ground.

- **Who doesn't yield.** `mask_view`, `edges_view` and `display_bloom` draw over everything (`StyleDescriptor::draws_over_entities()`): the debug views show the whole picture, and the glow crosses styles as light does. The `style` and `edges` debug modes don't run the pass.
- **The cut is the mask's.** The cut follows the mask pixel by pixel, so a glyph cell that straddles the cask's outline is cut along it. Whole-cell silhouettes ([#338](#whole-cell-silhouettes)) are a per-entity ASCII setting and don't apply to the edge of another style's entity.
- **The mask is drawn for it.** A style that yields asks for the mask while another running style owns an entity, even if it reads nothing else from the mask.
- **Cost.** The pass is one full-screen triangle per yielding style, drawn only on frames where another style owns an entity. It discards every pixel that isn't foreign.

### Bloom from a style

By default bloom's threshold reads the scene *under* the HDR styles: light comes from the world, not the brush, so a painted surface keeps the halo its original bright pixels had. A style whose own bright shapes should glow (ink brighter than white, a bleeding highlight) **feeds bloom** instead: bloom then reads the HDR stage's result (#300).

- A style says so in its descriptor (`feeds_bloom: true`) or, for a user style, in its `.shader.toml` (`feeds_bloom = true`, `stage = "hdr"` only).
- A scene overrides it in the style's table, Kuwahara's included:

  ```toml
  [post_process.styles.kuwahara]
  feeds_bloom = true
  ```
- On the command line: `--style kuwahara:feeds_bloom=1`, `--sweep neon.feeds_bloom=0,1`. In F4 it is the **Feeds bloom** checkbox under each HDR style in Styles, Kuwahara's included, and Copy TOML writes it when it differs from the style's default.
- When any HDR style that ran this frame feeds bloom, bloom reads the stage's final result, every style in it included. A display style can't feed bloom: it runs after it (`feeds_bloom` on one is a warning). ASCII's phosphor halo is its own `glow` for that reason.

Kuwahara's default is unchanged: off.

| ![neon isolines, bloom reading the original scene: the lines are flat, the lamp's halo glows where the style dimmed it](../images/bloom-follows-style-neon-off.png) | ![neon isolines feeding bloom: each line glows](../images/bloom-follows-style-neon-on.png) |
|---|---|
| `examples/neon_bloom.scene.toml --style neon:feeds_bloom=0`: bloom reads the scene under the style. The lines are flat, and the lamp, which the style dimmed to a fifth, still haloes at full strength | The scene as written (`neon` says `feeds_bloom = true`): bloom reads the styled picture, so each line glows and the lamp's halo shrinks with it |
| ![Kuwahara, bloom from the original scene](../images/bloom-follows-style-kuwahara-off.png) | ![Kuwahara feeding bloom](../images/bloom-follows-style-kuwahara-on.png) |
| Kuwahara at its default (`neon_bloom` with `--style neon:off --style kuwahara:radius=8 --exposure 2.2`, a crop at 2x): the lamp's halo keeps the original, unpainted rim | The same with `kuwahara:feeds_bloom=1`: the halo follows the painted strokes, so the rim breaks where the filter broke it. Subtle at Kuwahara's sizes, which is why it stays off by default |

## Writing a style

A style is one module in `crates/flint-render/src/stylize/` plus its name in the `register_styles!` list at the bottom of `stylize/mod.rs`. A game writes one the same way in its own crate and registers it at run time instead (#419): `flint_render::stylize::register(&STYLE)` before the first scene is read, or `PlayerExtension::styles` for a game that embeds the player ([Player Extensions](../guides/player-extensions.md#styles)). Everything below is public, so a game's style is built from the same parts; `--list-styles --json` gives it `"source": "game"`. The module exports a `StyleDescriptor` called `STYLE`, built with `const` constructors (#366) so that a field added to the descriptor, a parameter or an option later gets its default in the constructor and no style has to change (the types are `#[non_exhaustive]`, so a game crate can't write a literal either):

```rust
pub(super) static STYLE: StyleDescriptor = StyleDescriptor::new(
    "invert",                      // the [post_process.styles.<name>] key
    "Invert",                      // F4 and --gpu-time
    StyleStage::Display,
    create,                        // fn(&StyleDevice) -> Box<dyn StylePass>: pipelines in d.format
)
.params(&[StyleParam::new("strength", 1.0, 0.0..=1.0)])
.hints(&[StyleHint::new("strength", "Strength", "0 = none, 1 = the full negative")]);
// Everything else has a default, and a setter for a style that differs:
//   .options(&[StyleOption::new(name, default, choices)])   named strings;
//       .discovered(fn) finds choices beside the scene (#322)
//   .uses_mask(MaskUse::Always)    the renderer draws the entity style mask for it;
//       MaskUse::When(fn(&config, &ids) -> bool): only while it says so (#350)
//   .needs(&[StyleNeed::Edges])    the shared geometry edges (#301)
//   .component("ascii_style")      entities take the style per entity (#266)
//   .feeds_bloom(true)             bloom reads this (hdr) style's result (#300)
//   .blends_itself()               it draws its entities' blend itself, so the
//       framework's cross-fade leaves it alone (ASCII's dissolve, #396)
```

An option's `choices` are a closed list unless it is `discovered`: its `fn(&ChoiceScope) -> Vec<String>` then gives the list wherever a scene is known (`StyleOption::choices_in`, which the F4 menu and `--list-styles` read), from `scope.files(AssetKind::Palettes)` (every file of the kind the scene can see, by `Project::list` from the scope's `AssetScope`, through the vfs; a scope with no scene offers what is built in alone) and the other options' values (`(scope.option)("font")`). A discovered option is open: `--style ascii:palette=tidepool` passes, and the style resolves the name and warns if it can't.

`hints` are optional, and F4 draws a style without them: a setting with no hint shows its own name (`edge_max` becomes "Edge max") on a plain slider. A hint gives a setting (or a colour, by its key) a label, a hover line, and `.integer()` for a value that only makes sense whole (ASCII's cell size). `.when(option, &[values])` says the setting is read only while that option has one of those values, and `.unless(option, &[values])` that it is idle while it does; F4 greys the row out while a condition fails (a hint holds up to two). ASCII's are

```rust
StyleHint::new("tint", "Tint", "The ink for colour mode tint").when("color", &["tint"]),
StyleHint::new("background", "Background", "How much of the scene shows behind the glyphs (dim ground)")
    .when("ground", &["dim", "inverse"])
    .unless("color", &["palette"]),
```

A greyed row's reset button stays live, and the value is kept: it only stops mattering. `StyleDescriptor::unused_reason(config, name)` gives the hover line, or `None` while the setting is read.

### Colours

A colour is three parameters named `<base>_r`, `<base>_g` and `<base>_b`, each 0--1 in display values (what the eye sees, sRGB-encoded), and the framework makes them one setting (`StyleColor`, #315) everywhere a person or an agent meets it:

| Where | What it looks like |
|---|---|
| Scene table, `ascii_style`-type components | `tint = [1.0, 0.7, 0.25]` (integers allowed) or `tint = "#ffb340"`; a channel on its own (`tint_g = 0.5`) still works |
| `--style` | `ascii:tint=#ffb340` or `ascii:tint=1/0.7/0.25` |
| F4 → Styles | one colour picker |
| Copy TOML (`stylize::style_toml`) | `tint = [1.0, 0.7, 0.25]`, only when it differs from the default |
| `--list-styles` | one row, `tint  [0.35, 1, 0.45]  colour`; `colors` in the JSON |
| Scripts' `set_style_param` | `set_style_param(id, "tint", [1.0, 0.7, 0.25])` or `"#ffb340"`, clamped to the channels' ranges |
| The style, in `encode` | `frame.color("tint")` → `[f32; 3]`; per entity, `frame.variant_color(v, "tint")` |

Underneath they stay three parameters, so a per-entity variant row or a uniform reads three numbers as before and a style declares nothing new. A user-authored style gets the same by naming three `[params]` that way. `StyleDescriptor::colors()`, `color_setting(base)` and `color_of(param)` find them; `stylize::color_from_toml` / `color_from_text` parse the two spellings.

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

A style's WGSL goes into `kit::shader` as `kit::wgsl!("common.wgsl", "mine.wgsl")`: the files' `include_str!`s concatenated, plus their names. The names are what let a shader be tuned without a rebuild (#418). With `FLINT_STYLE_SHADER_DIR=crates/flint-render/src/stylize` set, natively, every built-in style reads its files from that folder rather than from the binary, shared files such as `ascii_common.wgsl` and `../palette.wgsl` included. The viewer and the player build again each style whose file was saved, a quarter-second poll on the frame thread, and `flint render` reads the folder on every run:

```bash
FLINT_STYLE_SHADER_DIR=crates/flint-render/src/stylize flint edit examples/watercolor.scene.toml
```

Saved in `watercolor.wgsl`, a tweak showed in the open viewer 470--672 ms later (nine edits). A debug rebuild after the same tweak took 35 s, and the release rebuild #272 measured took 3--7 minutes, each followed by a restart. naga checks what is read first. A file that doesn't parse or validate goes to the style error card as `file:line:col` with the line quoted, as a user style's does, and the shader last built from disk keeps drawing (else the compiled-in one). A shader naga takes and wgpu refuses, such as a renamed entry point, panics while the style is built. The style is then built from the last good shaders, and the card says so. Saving a good file clears the card. The variable is read once at start-up, which also logs a warning that it's on. It does nothing on the web, and a shipping build never reads the files.

What a style gets from `StyleFrame`:

| | |
|---|---|
| `width`, `height`, `format`, `texel_size()`, `srgb()` | the stage's size and format |
| `param(name)` | the style's parameter from the config, or its default |
| `option(name)` | the style's option (a string) from the config, or its default |
| `color(name)` | the style's [colour](#colours) `name` (`tint`) as `[r, g, b]` in display values, each channel from the config or its default (#315) |
| `pixels_per_point` | the window's scale factor (1 headless, or `flint render --pixels-per-point`), for styles sized in points |
| `scene_dir` | the scene's folder when the host knows it, for styles that load files (ASCII's glyph sets) |
| `depth`, `camera` | the scene's depth (sample by UV) and the inverse view-projection, position, near and far |
| `mask`, `mask_bind(...)`, `style_id(name)` | the [entity style mask](#the-entity-style-mask), while a `uses_mask` style is on |
| `edges`, `edges_bind(...)` | the [shared geometry edges](#shared-edges), while a style that `needs` them is on |
| `target(name, format)` | a texture of the style's own at the stage's size, kept until the next resize |
| `sampled(...)`, `sampled_input(...)`, `uniform(...)`, `bind(...)`, `depth_bind(...)` | bind groups kept with the post chain |
| `first_write(name)` | true once per chain, for uniforms that only change on resize |
| `time` | game seconds, for animated styles |

What the framework does so a style doesn't have to:

- **Lazy building.** A style's pipelines are built the first frame it is on, under `catch_unwind` (some drivers crash compiling shaders); one that fails stays off. Hosts can call `SceneRenderer::ensure_style_resources` after setting a config to move the compile off that frame.
- **Resize.** Every target and cached bind group lives in the post chain's resources, which `resize_postprocess()` rebuilds. There is no per-style resize hook to forget.
- **Formats.** HDR-stage styles draw in `Rgba16Float`; display-stage styles in the surface format. On an sRGB surface a display style samples linear values and its writes are encoded, so a style that works in display values decodes and encodes itself (`frame.srgb()`; `invert.wgsl` shows how).
- **Samplers.** Styles bind the chain's one linear sampler, so their bind groups add nothing to DX12's 2048-slot sampler heap.
- **The mask.** A `uses_mask` style gets the entity style mask drawn for it each frame, and no style pays for it otherwise. `MaskUse::When` asks per frame from the config and the ids (#350): ASCII reads it only while `edges > 0` and something is styled, or another style's entity passes through it, so `--ascii-no-edges` in a scene with no `style_mask` makes no mask target and `--gpu-time` shows no "Style mask" line.
- **Shared inputs.** A style that lists `StyleNeed::Edges` in `needs` gets the [geometry edge texture](#shared-edges), drawn once a frame however many styles read it.
- <a id="blend-for-free"></a>**Blend.** A style that reads the mask (`uses_mask`, or a per-entity `component`) fades with its entities' [`blend`](#blending-in-and-out) and the [veil](#whats-in-front-the-veil) in front of them without a line of blend code (#396). Right after its pass the framework draws a cross-fade into its output: one full-screen triangle, alpha-blended, that brings the style's input back by `1 - blend` wherever the mask's id belongs to this style and `.g` is below 1, and is discarded everywhere else. It needs no target (the chain's ping-pong already holds input and output), costs one mask read and, on fading pixels, one input read and a blend, and is drawn only while some entity's blend is below 1 or a veil covers a styled one, so a fully styled scene draws it not at all (`SceneRenderer::style_fade_runs` counts). The mix is the blend unit's: linear light in the HDR stage and on an sRGB surface. Which style an id belongs to is a storage table written once a frame (id to registry index), and which style just drew is the draw's instance index, so one table serves every style in both stages. A style that does better than a cross-fade opts out with `.blends_itself()` and reads `.g` itself: ASCII does, for its display-space `fade` and its cell-by-cell `dissolve`.
- **Timing.** Each style is a GPU-timer scope under its `label`, so `flint render --gpu-time` and the player's `--perf` name it.
- **The command line.** `flint render --style <name>` reaches a new style from its descriptor alone.

## User-authored styles

*Experimental, format version 1 ([ADR 0112](../../../adr/0112-user-authored-styles.md); #274, #275). `flint render`, `flint edit`, `flint-player`, `flint bundle` and the web player all load them.*

A game can write a style without touching the engine: a `styles/<name>.shader.toml` and one WGSL function beside it. Flint finds them by [the one asset search](assets.md#where-the-engine-looks-for-a-file) (kind `styles`): every `*.shader.toml` in the `styles/` beside the scene, in each directory above it up to the project root, and in the mounted engine. So a game with `levels/` and `styles/` side by side needs nothing else, and neither does a scene three folders down. Of two styles of one name the nearest to the scene wins, whichever scene loaded first: when a later scene's nearest is another file, it takes the name over in place. A style's WGSL and images are read from beside its `.shader.toml`, wherever that was found. Start one with `flint style new` (#416), from the game's folder:

```bash
flint style new dusk                         # styles/dusk.shader.toml + styles/dusk.wgsl
flint style new dusk --stage hdr --inputs depth,mask --dir styles
flint style check styles/dusk.shader.toml    # no GPU; exit 1 on an error
flint style check styles                     # every *.shader.toml in a folder
flint style prelude styles/dusk.shader.toml  # the generated code the WGSL is compiled inside
flint style prelude styles/dusk.shader.toml --composed   # ...with the WGSL and entry points
flint style list levels/a.scene.toml         # the same list as flint render --list-styles
flint render levels/a.scene.toml --style dusk:amount=1
```

`new` writes a commented passthrough: its header lists every field of `p` and `flint`, and every helper the chosen stage and inputs give (and which inputs would give more), with commented-out calls to the input helpers and one example parameter, `amount`, that does nothing at its default (so the new style renders byte-identical to no style until you change it). It refuses a built-in's name, a name that isn't lowercase letters, digits and `_`, and files that are already there (`--force` overwrites them), and it checks what it wrote. `check` runs the same naga check the hosts run on load, prints `file:line:col` errors with the line quoted (and the hint for a helper of an undeclared input) and the loop and texture-read warnings, and exits 1 if any style failed: the fast loop for an agent, or for CI. For a longer worked example, `examples/styles/template.shader.toml` and `template.wgsl` are a split tone.

```toml
# styles/greyscale.shader.toml
version = 1
name = "greyscale"         # the file stem; lowercase, digits and _
stage = "display"          # or "hdr"
shader = "greyscale.wgsl"
inputs = []                # any of "depth", "mask", "edges"; colour is always there
# feeds_bloom = true       # hdr only: bloom reads this style's result (#300)

[params.amount]
default = 1.0
min = 0.0
max = 1.0
label = "Amount"           # optional, with help = "..." and integer = true
```

```wgsl
// styles/greyscale.wgsl
fn style(p: StylePixel) -> vec4<f32> {
    let grey = vec3<f32>(flint_luma(p.color.rgb));
    return vec4<f32>(mix(p.color.rgb, grey, params.amount), 1.0);
}
```

It is then a style like the built-ins: `[post_process.styles.greyscale]` turns it on, `--style greyscale:amount=0.5` and `--sweep greyscale.amount=0,0.5,1` reach it, and the F4 menu draws its sliders (`<base>_r/_g/_b` params become one colour picker).

Flint writes everything around the function: the bindings, a `params` struct with one `f32` per parameter, and the entry points. `p.uv` is 0..1 from the top left, `p.pos` the pixel, `p.color` the input there. At the display stage colours in and out are display values on any target, so a style never decodes sRGB itself. At the hdr stage they are linear HDR.

| | |
|---|---|
| `flint.resolution`, `flint.texel`, `flint.time`, `flint.frame`, `flint.pixels_per_point` | the frame |
| `flint.near`, `flint.far`, `flint.camera_pos`, `flint.inv_view_proj` | the camera |
| `flint_sample(uv)` | the input anywhere, in the same space as `p.color` |
| `flint_luma(c)`, `flint_hash(v)`, `flint_to_display(c)`, `flint_to_linear(c)` | small helpers |
| `flint_depth(uv)`, `flint_view_depth(uv)`, `flint_world_pos(uv)` | with `inputs = ["depth"]`: raw depth, metres from the eye, the world position |
| `flint_mask_id(uv)`, `flint_style_blend(uv)` | with `inputs = ["mask"]`: the [style mask](#the-entity-style-mask) id, and 0..1 for entities whose `style_mask` or `<name>_style` names this style (their `blend`), 0 elsewhere and on an opt-out. A shader that never calls `flint_style_blend` is [cross-faded](#blend-for-free) by the framework; one that calls it draws the blend itself and isn't (decided when the `.shader.toml` is read, so adding or removing the call takes effect at the next load or `.shader.toml` edit) |
| `flint_params_at(uv)` | with `inputs = ["mask"]`: the settings of the entity at `uv`, for reading a neighbour's (`params` is already this pixel's) |
| `flint_edges(uv)`, `flint_edge(uv)` | with `inputs = ["edges"]`: the [shared edges](#shared-edges) (silhouette, crease, direction), and the stronger of the two strengths; 1 is an edge |

**Per entity (#414).** A style with `"mask"` in its `inputs` is per-entity, like ASCII, pencil and watercolor. A `style_mask { style = "<name>" }` on an entity switches it on with no `[post_process.styles.<name>]` table, and the style gets a component of its own, `<name>_style`, whose fields are its parameters (plus `enabled` and `blend`, as on `ascii_style`). Each distinct set of settings is a variant with its own mask id, and the framework uploads a row of `Params` for each, so in the WGSL `params` holds the settings of the entity the pixel shows, each field falling back on the scene's table, then the default. A pixel that no entity of the style covers reads the scene's row. `enabled = false` opts an entity out: its pixels read `flint_style_blend` as 0. Scripts reach the component the way they reach ASCII's: `set_entity_style(id, "hologram")`, `set_style_param(id, "glow", 1.5)`, `tween_style_param`.

```toml
[entities.ybot.style_mask]        # on, with the scene's settings (or the defaults)
style = "hologram"

[entities.barrel.hologram_style]  # on, with its own
glow = 2.0
lines = 6.0
blend = 0.6
```

One frame holds 15 variants of a style (and mask ids up to 255); any more take the scene's settings, with a warning. The component has no schema file, so the scene loader neither fills in defaults nor checks its fields' types. A style that doesn't read the mask can't tell entities apart, so it stays whole-frame: only its table switches it on, and a `style_mask` naming it does nothing. The same holds for the built-ins without a component (`invert`, `mask_view`, `edges_view`, `palette`).

| ![two holograms, one frame](../images/user-style-hologram-per-entity.png) |
|---|
| `examples/user_styles.scene.toml` with no hologram table. The YBot's `style_mask` takes the defaults (`glow = 0.8`, 3-point lines). The barrel's `hologram_style` sets a brighter rim and wider lines at `blend = 0.6` |

**When it goes wrong.** Flint checks the composed shader with naga before the GPU sees it. An error names your file, line and column, quotes the line, and gives a hint if you called a depth or mask helper without declaring the input. A style that doesn't compile still loads and draws its input unchanged, so the scene keeps its other styles.

**Loops are guarded.** Natively, every loop in your code gets a counter: one run of a loop turns at most `LOOP_LIMIT` = 4096 times, then breaks out. A loop that ends by itself within that (a blur, a kernel, a march of a few hundred steps) draws exactly as written. A loop that never ends draws whatever its pixel had reached after 4096 turns, a wrong colour you can see and fix, instead of hanging the GPU until the driver resets it. Each loop counts on its own and starts again each time it is entered, so nested loops multiply (a 64x64 kernel is fine). Flint compiles naga's rewrite of your module, but errors still name the lines of your file. On the web the guard doesn't run yet ([ADR 0112](https://github.com/chrischaps/Flint/blob/main/docs/adr/0112-user-authored-styles.md)): keep loop bounds constant or a clamped parameter, as `examples/styles/radial_blur.wgsl` does with its `samples`.

**Hot reload.** Natively, both files reload while the player or viewer runs, a few times a second:

- Save the `.wgsl` and the next frames draw it. If the edit doesn't compile, the last good build keeps drawing.
- Save the `.shader.toml` and the style is replaced where it stands in the chain, with its new parameters, label, stage, inputs or shader. The scene's `[post_process.styles.<name>]` is read again against them (other F4 edits stay). A description that no longer reads keeps the old style and reports.
- A new `*.shader.toml` dropped into any `styles/` folder the scene searches is picked up too. One dropped nearer the scene than the style of its name takes over from it, and gives the name back when it is deleted.

Until a broken style compiles, a card in the bottom-left corner of the player and the viewer shows its error with the line it points at, and the style's F4 header is tagged `error` (hover it for the message; the `user` tag beside it names both files). `flint render` prints the same message and renders the passthrough. The goldens hold both of those to account (#417): `user-style-broken` renders a bench with a fixture style that fails (`crates/flint-cli/tests/golden/fixtures/styles/broken.*`), requires the frame to equal the same bench rendered without the style, and requires the error to name `broken.wgsl:9:9`. Next to it, `user-styles`, `user-style-risograph` and `user-style-contour-map` pin the three demo styles over `examples/user_styles.scene.toml`.

| ![a cyanotype from a game folder, drawn by flint-player](../images/user-style-cyanotype.png) | ![the same style mid-edit: a typo, the card, the last good build](../images/user-style-error-card.png) |
|---|---|
| A game outside the engine (`levels/`, `styles/cyanotype.*`) in `flint-player`, after its `.wgsl` was edited live | The next save had a typo (`params.exposre`): the card names `cyanotype.wgsl:7:57` and quotes the line while the last good build keeps drawing |

**On the web.** `flint bundle` packs `styles/` with the rest of the game. It loads every user style each scene sees from the packed files, the way the browser will, and refuses the bundle if a description doesn't read, its shader isn't packed or the shader doesn't compile. In the browser the style is checked by the browser's own compiler, and nothing reloads. The compiler answers a frame or two after the style is built: until then the style draws its input, and if the compiler refused it, the same card shows its error with the line in your file, and the console logs it (#415). The browser is stricter than the bundle check in one place: it refuses a derivative (`dpdx`, `dpdy`, `fwidth`) inside a branch on the pixel's own value, which naga lets through. Take the derivative before the `if`, and use it inside. (`flint_sample` reads at mip 0, so it is fine anywhere.)

![the web player at ?game=frost: the card names frost.wgsl:12:42, 'fwidth' must only be called from uniform control flow](../images/user-style-web-error.png)

| ![hologram](../images/user-style-hologram.png) `hologram`: `inputs = ["mask"]`; the YBot and the barrel carry `style_mask { style = "hologram" }`, the barrel at `blend = 0.6` (rendered before #414, with the table the scene then needed; [per entity](#user-authored-styles) shows it now) | ![contour map](../images/user-style-contour-map.png) `--style contour_map` (named `contour` until #599 gave the name to the built-in ink): `inputs = ["depth"]`; height lines from `flint_world_pos`, range rings round the eye |
|---|---|
| ![risograph](../images/user-style-risograph.png) `--style risograph` (a crop at 1:1): two dot screens out of register, colour only | Each is one `.shader.toml` and under 50 lines of WGSL in `examples/styles/`; none needed a rebuild of the engine |

### Images and options

*#413.* A style can read images of its own (a LUT, a paper, a brush or glyph sheet) and take string options. Both are declared in the `.shader.toml`:

```toml
# A fixed image, a file beside the .shader.toml (subfolders allowed, no ..)
[images.brush]
file = "brushes/dry.png"
srgb = false               # true: decode sRGB to linear as it's read (a colour picture for an hdr style)

# A choice: params.finish is a u32 index into the choices, and each choice
# that is a word gets a constant, FLINT_FINISH_MATTE = 0u, FLINT_FINISH_GLOSS = 1u
[options.finish]
default = "matte"
choices = ["matte", "gloss"]
label = "Finish"           # optional, with help

# An image the scene picks by file name: read as flint_image_paper(uv), and
# its index in params.paper. Without choices it offers every image in its
# default's folder (papers/ here), found when the style loads.
[options.paper]
kind = "image"
default = "papers/cotton.png"
```

The scene picks with strings, as for any built-in's options: `[post_process.styles.darkroom] paper = "papers/washi.png"`, `--style darkroom:finish=gloss`, `--sweep darkroom.grade=luts/neutral.png,luts/bleach.png`. F4 draws each option as a combo box of its choices. A value outside them is warned when the scene loads, naming the choices and the default it draws as (#318), and `--style` refuses it.

Each image `<name>` gets four helpers. They read through the input's sampler, so images add no sampler to the bind group (the DX12 heap, see Critical Gotchas):

| | |
|---|---|
| `flint_image_<name>(uv)` | bilinear, 0..1 across the image, clamped at its edges |
| `flint_image_tiled_<name>(uv)` | bilinear and repeating: uv 1.25 is 0.25, with no seam |
| `flint_image_size_<name>()` | its size in texels, e.g. to tile it at one texel a point: `p.pos / (flint_image_size_paper() * flint.pixels_per_point)` |
| `flint_lut_<name>(rgb)` | the image read as a 3D LUT strip: N slices of N x N side by side (N*N wide, N high), blue choosing the slice left to right, red across a slice and green down it, as Unreal's `RGBTable16x1.png` is laid out. Trilinear |

Images are PNG or JPEG, 8 bits a channel and one mip, read through the vfs (so a bundle carries them; `flint bundle` refuses one that names an image it didn't pack). Without `srgb`, the shader gets the values as stored: at the display stage those are display values like `p.color`, and a LUT wants them untouched. A style may bind up to 8 images, image options among them, in group 3. Every file an image may read must exist when the style loads; a missing one is reported, and until it reads the style draws its input unchanged.

A per-entity style (one with `inputs = ["mask"]`) takes options from each entity's `<name>_style` too, a row each. An image is one binding for the whole frame, so an image option always shows the scene's file, and an entity's value changes only its index in `params`.

Natively, images reload like the shader: save a PNG and the next poll (a few times a second) reads it again. A file that no longer decodes keeps the last good picture drawing and shows its error on the card.

`examples/styles/darkroom.*` uses all three: a LUT grade and a paper as image options (`luts/`, `papers/`, made by `tools/darkroom_images.py`), and a matte or gloss finish.

| ![darkroom on the user styles scene: warm print grade on cotton paper, with a white border](../images/user-style-darkroom.png) | ![four LUTs: neutral, warm print, bleach bypass, cyanotype](../images/user-style-darkroom-grades.png) |
|---|---|
| `--style darkroom`: the warm print LUT on cotton paper, matte | `--sweep darkroom.grade=...`: the four LUTs in `examples/styles/luts/` |

![The three papers across, matte and gloss down, over the neutral LUT](../images/user-style-darkroom-papers.png)

`--style darkroom:grade=luts/neutral.png --sweep darkroom.paper=papers/cotton.png,papers/kraft.png,papers/washi.png --sweep darkroom.finish=matte,gloss`: an image option and a choice option as the two axes of one sheet.

### Passes and targets of its own

*#412.* Four of the six built-ins need more than one pass: a blur is two (across, then down), a wash is painted and then read, ASCII keeps last frame's cells. A user style does this with `[[passes]]` in place of the top-level `shader`. Each pass is a WGSL `fn style` of its own, run in the order listed. Every pass but the last writes a **target** of the style's own, and the last writes the stage's output:

```toml
# styles/orton.shader.toml (examples/styles/orton.*): a blurred, overexposed copy screened over the frame
version = 1
name = "orton"
stage = "display"

[[passes]]
name = "expose"              # optional: pass1, pass2... by place
shader = "orton_expose.wgsl"
target = "exposed"           # what it writes
format = "rgba16f"           # or "rgba8" (0..1, stored as written) or "r32f" (one float)
scale = 0.5                  # of the stage's size, 0.0625..1 (default 1)

[[passes]]
name = "blur_x"
shader = "orton_blur_x.wgsl"
target = "wide"
scale = 0.5
reads = ["exposed"]          # earlier passes' targets

[[passes]]
name = "blur_y"
shader = "orton_blur_y.wgsl"
target = "soft"
scale = 0.5
reads = ["wide"]

[[passes]]
name = "sandwich"            # the last: no target
shader = "orton.wgsl"
reads = ["soft"]
```

A pass reads what it lists in `reads` with helpers the framework writes for each one:

| | |
|---|---|
| `flint_target_<name>(uv)` | the target at `uv`, filtered (nearest for `r32f`, which most web GPUs can't filter) |
| `flint_target_<name>_load(texel)` | one texel, clamped to the edge |
| `flint_target_<name>_size()` | its size in pixels |
| `flint.pass_size`, `flint.pass_index` | the size of what this pass draws (its target, or the output) and its place, 0 first |

A target holds what its pass returns, unchanged: there is no sRGB encode, so a display-stage pass that writes `p.color` into an `rgba16f` target and a later pass that reads it see the same display values. Only the last pass's result is encoded for the output. `params` are shared by every pass, and `inputs` (depth, mask, edges) reach every pass.

**History.** `history = true` on a pass's target keeps it from frame to frame. The framework keeps two textures and swaps them each frame, so a pass at or before the writer that reads the target gets **last frame's**, and passes after the writer get this frame's. A target can therefore read itself:

```toml
[[passes]]
name = "trail"
shader = "trail.wgsl"
target = "trail"
history = true
reads = ["trail"]            # last frame's trail
```

Last frame's is kept only if it is this picture's past, the rule ASCII's cells follow (#277). On a first frame, a frame after [`reset_style_history`](#temporal-stability) (a camera cut or a new scene), after a resize, or when the style was off the frame before, the history texture is cleared to transparent black and `flint.history` is 0. Otherwise it is 1. Fade with `mix(now, past, keep * flint.history)` and a cut starts clean.

**What is checked when the description loads** (each error names the passes):

- A read of a target before the pass that writes it: `pass "a" reads target "tb" before pass "b" writes it; move "a" after "b" (or add history = true to read last frame's)`.
- A cycle: `a cycle: passes "a" -> "b" -> "a" each read what the next writes`. A pass reading its own target without `history` is one too.
- The shape: a target on every pass but the last and none on the last; one writer a target; known formats; `scale` within 0.0625..1; no pass reading a target nobody writes. A target nobody reads loads, with a warning.
- **The caps.** At most 8 passes and 4 targets, and together the targets may cost no more than four full-size `rgba16f` targets: 32 bytes for each pixel of the stage, a history target counting twice. That is 66 MB at 1920x1080. The orton demo's three half-size targets cost 6 bytes a pixel (12 MB at 1080p).

A shader that doesn't compile is reported as before, with its own file, line and column. A helper of a target the pass doesn't read gets a hint (`add "soft" to reads of pass "blur_y"`). Until every pass has compiled once the style draws its input unchanged, and after that a broken edit keeps that pass's last good build. Each pass's `.wgsl` reloads on its own.

**What it costs.** `flint render --gpu-time` times the style as one row and then lists every texture of the style chain with its size, format and bytes. A user style's targets are named `<style>.<target>`, with `.a`/`.b` for a history pair:

```text
  Hologram       0.012 ms
  Orton glow     0.032 ms
  frame          0.912 ms
Style targets: 7 (16.70 MB)
  orton.exposed          640x360   Rgba16Float        1.76 MB
  orton.soft             640x360   Rgba16Float        1.76 MB
  orton.wide             640x360   Rgba16Float        1.76 MB
  style display in      1280x720   Rgba8UnormSrgb     3.52 MB
  ...
```

That is `examples/user_styles.scene.toml --style orton --gpu-time 240` at 1280x720 on an RTX 4070: the four passes of the Orton glow together cost 0.032 ms.

The F2 overlay's `Style targets` line includes them in its total. F4's `user` tag lists each pass's file and each target when you hover it.

![examples/user_styles.scene.toml under --style orton: the yard's light blooms softly, edges stay crisp](../images/user-style-orton.png)

**Passes, images and per-entity settings together.** A pass that reads targets binds them in group 3 after the style's [images](#images-and-options), so `flint_image_<name>` and `flint_target_<name>` work side by side in any pass. Every pass reads the same `params`, and for a per-entity style (`inputs = ["mask"]`) that is the pixel's own entity's settings in every pass.

**What a user style can't do yet:** compute passes, mip chains, or targets whose size isn't a fraction of the stage's. The built-ins that need those stay in Rust. The demos in `examples/styles/` show what user styles reach: `risograph` (two misregistered inks), `contour_map` (survey lines from depth; `contour` before #599), `darkroom` (a LUT and a paper picked by the scene), `neon` (an hdr style that feeds bloom, `examples/neon_bloom.scene.toml`), `hologram` (a style mask on chosen props, `examples/user_styles.scene.toml`) and `orton` (four passes through three half-size targets).

## Goldens

Every registered style is pinned by a golden image in `crates/flint-cli/tests/golden/`, listed in `stylize.goldens.toml` (#271). `flint goldens` renders each case as `flint render` would, all in one process (#329), and fails when one moves past its tolerance; `cargo test -p flint-cli --test render_goldens` runs the same suite. CI runs it on WARP for every PR (#314), against a `<stem>.warp.png` variant where WARP's render differs from the golden by more than the tolerance allows ([adapters and variants](../guides/headless-rendering.md#adapters-and-golden-variants)). Each run leaves an `index.html` contact sheet in its out dir, failures first, with golden, render and diff side by side ([the contact sheet](../guides/headless-rendering.md#the-contact-sheet)). Review a `--bless` there.

`--case` takes a case's name or part of one. An exact name picks that case alone: `--case ascii-silhouette-cell` blesses `ascii-silhouette-cell` and leaves `ascii-silhouette-cell-object` as it was. A part that names no case picks every case containing it, so `--case ascii-silhouette` runs both. After blessing a single case, run the whole suite before committing.

```bash
flint goldens crates/flint-cli/tests/golden/stylize.goldens.toml            # gate
flint goldens crates/flint-cli/tests/golden/stylize.goldens.toml --case ascii-silhouette-cell
flint goldens crates/flint-cli/tests/golden/stylize.goldens.toml --bless    # after a deliberate change
flint goldens crates/flint-cli/tests/golden/stylize.goldens.toml --bless-drifted  # catch up passes that moved
```

A run names every case that passes without being byte-identical with its golden, or says there are none ([drift](../guides/headless-rendering.md#drift), #387).

| Case | What it pins |
|------|--------------|
| `ascii-full` | `--style ascii:edges=0` on `pbr_test`: the #265 golden, unchanged with edges off, and its character grid exactly (`ascii_pbr_test.txt`, #317) |
| `ascii-full-detailed-tint` | another ramp and the tint colour mode on `stylize_props_ascii`, with edges |
| `ascii-full-hidpi` | `--pixels-per-point 2`: cells in points (edges off) |
| `ascii-full-msaa4` | `--msaa 4` scene passes under the glyph grid (edges off) |
| `ascii-edges-pbr` | [edges](#edges) on `pbr_test`, grid off |
| `ascii-edges-pbr-grid` | the same with the editor grid on: the grid writes no depth, so no edge traces it (#346) |
| `ascii-edges`, `ascii-edges-msaa4`, `ascii-edges-scale05` | the styled sphere and box of `examples/ascii_edges.scene.toml` outlined, at 1x, under MSAA 4 and at `--render-scale 0.5` |
| `ascii-entity`, `ascii-entity-object` | the spike's per-entity modes (#261), screen and object grids |
| `style-mask` | the entity style mask the per-entity styles read (`--debug-mode style`) |
| `styles-mixed` | ASCII and pencil props in one frame, on the mixed bench: each style leaves the other's pixels alone (#386) |
| `edges-view`, `edges-inkline-scale05` | the [shared edges](#shared-edges) on the props scene, and a user style reading them beside `edges_view` at `--render-scale 0.5` |
| `invert`, `kuwahara` | the other styles, with a tighter tolerance |
| `contour`, `contour-over-kuwahara`, `contour-flags-are-its-table`, `contour-blend-zero` | the ink alone and over Kuwahara's paint (#599), the old `--contour-*` flags against the table they set, and a blend of 0 against no ink at all |
| `palette-phosphor-dither`, `palette-cga16` | the [palette style](#palette) on the props scene: a ladder dithered in 2-point blocks, and nearest colours flat. Glyph tolerance, since a colour on a boundary flips a whole block |

Cases render single-sample with the post clock at 0 unless they say otherwise. The props cases share one clearing, `examples/prefabs/stylize_props.prefab.toml`, which loads in a few seconds in a debug build, with the grid off. Each style has a bench of its own on it (#386): `stylize_props_ascii` (the barrel and YBot in glyphs), `stylize_props_pencil` (the cask and crate sketched) and `stylize_props_plain` (nothing styled, for whole-frame styles and looks), so a change to pencil moves only the pencil cases and `look-graphite`. The mixed bench, `stylize_props.scene.toml`, keeps both styles for the mask cases and `styles-mixed`, the one case that moves with either: it holds that each style leaves the other's pixels alone. A new per-entity style adds a bench of its own (a scene placing the prefab, with overrides giving its props a `style_mask`); a whole-frame style or a look goes on the plain one. Glyph styles share #265's tolerance (fail when more than 2% of pixels differ by more than 48 levels, or the mean passes 3), since a cell on a ramp step's border can flip glyph on another GPU; smooth styles take `level = 8, share = 0.003, mean = 0.25`, because Kuwahara at radius 4 against a radius-5 golden is a 0.39 mean the glyph limits would pass. On one machine, every case is byte-identical run to run. **A new style adds a case here.** The manifest format is in [Headless Rendering](../guides/headless-rendering.md#golden-suites).

## Shared edges

Line styles are defined by their edges, and the edges that read best come from the geometry, not the picture's colour: a wood texture makes none, while a table's outline and the creases where its faces meet do (the ASCII spike's round 2, #261). Finding them costs a 5x5 patch of positions unprojected from the depth for every pixel, so the framework does it once a frame (#301) and hands the result to every style that asks. A style asks in its descriptor, `needs: &[StyleNeed::Edges]`, and reads `frame.edges` (bind it with `frame.edges_bind(...)` over a `kit::texture_layout`); a [user style](#user-authored-styles) adds `"edges"` to its `inputs` and calls `flint_edges(uv)`.

The texture is `Rgba16Float` at the depth's size (the scene's, so under `--render-scale` a display style reads it by UV, like the depth and the mask):

| channel | holds |
|---|---|
| `r` | silhouette strength: a 3x3 Sobel of the view distance relative to the centre pixel's (so a far edge counts as much as a near one), kept on the near side only, over `edge_depth` |
| `g` | crease strength: the Sobel of normals rebuilt from the depth, counted only where the 5x5 patch is one surface (across a depth step rebuilt normals mark a band on both sides), over `edge_normal` |
| `ba` | the unit direction across the edge in screen space (x right, y down): the silhouette's where `r >= 1`, else the crease's, picked from both axes of the normal Sobel |

A strength of 1 is an edge at the scene's thresholds. The two pixels at each border are always 0, where the patch clamps and makes a degenerate normal.

```toml
[post_process]
edge_depth = 0.6     # the relative view-distance step that is a silhouette
edge_normal = 2.5    # the normal change that is a crease
```

The thresholds are shared because the pass is: every style reading the edges sees the same strengths. A style that wants more or fewer lines compares them against a level of its own (`examples/styles/inkline.wgsl` multiplies them by its `reach`). `flint render --edge-depth` / `--edge-normal` set them for one render, and F4's Styles section has them under **Shared edges**.

| ![edges_view on the props scene: the scene in grey, every edge pixel a hue for its direction](../images/shared-edges-view.png) | ![inkline, a user style with inputs = ["edges"], over the same scene](../images/shared-edges-inkline.png) |
|---|---|
| `--style edges_view`: the texture itself. Hue is the direction (opposite sides of a line agree), silhouettes are solid, creases softer; `ghost` shows strengths just under the threshold while tuning | `--style inkline` (`examples/styles/inkline.*`): flat paper tones under ink from `flint_edges`, about 40 lines of WGSL. Here it runs after the scene's own ASCII and pencil props |

At 1920x1080 on an RTX 4070 the pass costs about 0.16 ms, once, with one or two styles reading it (`--gpu-time` names it **Style edges**); `edges_view` and `inkline` add 0.04 ms each. While no style that needs the edges is on, the pipeline isn't built and nothing is drawn. The ASCII and pencil styles still find their own edges (ASCII's also outlines its own mask ids, pencil's adds luma edges); moving them onto the shared texture is still to do.

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

![The style mask debug view over examples/style_mask.scene.toml: the bench and the skinned YBot in one hue ("ascii"), the barrel, the crate and the lantern it carries in another ("pencil"), the opted-out box on the crate grey, and the legend in the lower left naming each hue](../images/style-mask-debug.png)

See it with the `style` debug view, which switches the `mask_view` style on and runs it alone: while the view is up the scene's other styles, of either stage, are suspended, so the hues lie over the plain scene rather than over glyphs or hatching (#334). A scene that turns `mask_view` on in its own `[post_process.styles]` keeps its chain, the mask view reading whatever ran before it:

```bash
flint render examples/style_mask.scene.toml --debug-mode style --output mask.png
```

Each style is a hue over the scene's shading, unstyled pixels are grey, and a white line marks where two ids meet. A legend in the image's lower left gives each id's swatch and name, and the command prints the same table (#309):

```text
style mask: 1 ascii (blue), 2 pencil (lime)
```

Ids follow name order, so they shift when a scene adds a style: read the legend rather than remembering a hue. A variant is its own row (`ascii{ramp=blocks}`, `ascii@barrel`). In the player and viewer the view is **Style Mask** in F4's debug shading list, and choosing it shows the same legend under the list; `flint_render::style_mask::legend` / `legend_line` build it from `SceneRenderer::style_ids()` for any other host.

How it's drawn:

- **A redraw, not a new attachment.** After the scene pass, each visible opaque draw of a styled entity is drawn again into the mask with its cached transform bind group (and bone bind group for skinned meshes). The scene pipelines don't change and the per-entity loop makes no buffers or bind groups; the ids go up in one write a frame.
- **Depth without a depth buffer.** A mask fragment tests itself against the scene's single-sample depth with a tolerance of twice its own depth slope, so grass or a wall in front of a styled prop cuts its mask. Under `--render-scale` the mask is at the scaled size, and a display-stage style reads it by UV.
- **MSAA: drawn 4x, resolved "any sample wins"** (#306). Under `--msaa 4` the mask is drawn into a 4x target of its own, and each sample the triangle covers is tested against that sample of the scene's multisampled depth. A resolve then folds the four samples into the single-sample mask the styles read: a pixel takes the id that covers the most samples, and is empty only if no sample was styled. Drawn single-sample at pixel centres, a feature about a pixel thin (the bench's seat edge at a grazing angle) lost the pixels the scene's samples gave it, and an outline style drew the gaps as white dashes. The 4x target costs 33 MB at 1080p, and only while MSAA and a mask-reading style are both on.
- **Only when wanted.** The mask target and its pipelines exist only once a style whose `uses_mask` asks for it this frame (or the debug view) is on. At 1080p on an RTX 4070 the pass (with the veil) costs about 0.022 ms for `examples/style_mask.scene.toml`'s styled entities, or about 0.05 ms under MSAA 4 with the 4x draw and its resolve, some 0.025 ms more than the single-sample mask cost there before #306.
- **What's masked.** Static glTF meshes, procedural boxes, skinned meshes, terrain (#310) and styled transparents (#310, below). Particles, sprites, grass and water never are: grass is instanced, and the rest write no depth to test against.

### Terrain and transparents

A heightmap terrain is masked like any mesh when its entity carries a `style_mask` or a style's component (`ascii_style`, `pencil_style`), or inherits one. Each visible chunk is redrawn into the mask with the terrain's own transform, so an ASCII or pencil ground runs under smooth props, and every unstyled prop on it cuts its own hole, pixel by pixel, from the depth test:

```toml
[entities.ground]
archetype = "terrain"
[entities.ground.ascii_style]
color = "tint"
tint = [1.0, 0.72, 0.32]
```

The host names the terrain's entity (`SceneRenderer::set_terrain_entity`) beside `load_terrain`; `flint render` and the player do.

**A styled transparent is a lens.** Glass, a ghost or a hologram with a style of its own is drawn into the mask after every opaque, back to front as the scene drew it. It writes no depth, but it is tested against the depth the opaques wrote, so an opaque in front still hides it. Where it shows, its id (and its blend) replaces whatever was behind it, and the style draws that whole pixel: what you see through a green-glyph pane comes out in green glyphs. The decision, and the alternatives it was chosen over:

- *Skip styled transparents* (the mask before #310). A pane that asks for a style would draw as plain glass, which surprises whoever asked.
- *Mask by coverage.* A half-covered pixel would be a share of the glass's style and a share of what's behind. One id per pixel can't carry two styles, and the coverage would need the material's alpha (texture and all) in the mask pass itself; the veil below samples it only for unstyled transparents. The blend channel already gives a lens a softer edge: set `blend` below 1 on the pane to mix the style with the plain view through it.

What the lens costs: the pixel the style redraws is the composite (glass and all behind it), and a transparent cut-out (an alpha-textured leaf card drawn blended) masks its whole quad, since the mask pass doesn't sample the material. Unstyled transparents in front of a styled entity, the lens included, are the veil's (below), the other side of the same question. The veil is depth-tested only against opaques, so an unstyled transparent *behind* a styled pane also thins the pane's blend where they overlap.

![examples/style_terrain.scene.toml: amber ASCII hills under a smooth barrel and table, and a green glyph pane that draws the half of the table behind it, and the hill, in its own style](../images/style-terrain.png)

### What's in front: the veil

Transparents and particles write no depth, so the depth test alone can't see them: where fire or glass sits in front of a styled stone, the stone's id would stay in the mask and the style would draw the fire as if it were stone. The rule (#290) is that **what's in front unmasks by its coverage.** Just before the mask, a veil pass draws every visible transparent mesh (static and skinned) and every particle into an `R8Unorm` target, depth-tested against the scene and never writing it, with the alpha it is drawn with (an additive particle covers by its alpha times its brightest channel). Layers blend "over" each other, so two faces of 50% glass cover 75%. The mask then writes each styled pixel's blend times what the veil leaves uncovered.

![Before #290: the campfire in front of the glyph stone is turned into stone glyphs, and the glass in front of the crate disappears into the crate's](../images/style-veil-before.png)

![After: the fire draws as fire over the stone, and behind the 45% glass the crate's glyphs show through at the share the glass leaves](../images/style-veil-after.png)

```bash
flint render examples/style_veil.scene.toml --particle-time 2 --output veil.png
```

- Every mask-reading style honours the blend, so it shows the fire as fire and the stone through any uncovered share: ASCII and user styles that call `flint_style_blend` by their own code, and pencil, watercolor and the rest through the framework's [cross-fade](#blend-for-free) (#396). A fully covered pixel keeps its id at blend 0, so under a whole-frame ASCII the stone behind thick smoke is a smooth hole, as a blend-0 entity is.
- A transparent that is itself styled stays out of the veil: it is in the mask as a lens ([above](#terrain-and-transparents)).
- The pass costs about 0.02 ms at 1080p on an RTX 4070 for `examples/style_veil.scene.toml` (69 particles and a pane), and is skipped in a frame with no styled draws.

### The style albedo

A display-stage style sees the finished picture, which is each surface's colour times its light. A style that wants the light alone (pencil's `tone = "light"`, #869) asks for the **style albedo**: `.uses_albedo(MaskUse::When(f))` (or `Always`) on its descriptor. While such a style is on, the renderer redraws the frame's opaque static, procedural and skinned draws once more after the scene pass, with their cached transform and material bind groups and the scene's own vertex stages, through a fragment stage (`fs_albedo` in `shader.wgsl` and `skinned_shader.wgsl`) that builds only the base colour (vertex colour and tint, base texture, the glowing share of a vertex-emissive surface taken out) and no light. It writes an `Rgba8Unorm` target at the depth's size: linear rgb, and alpha 1 where a mesh was drawn. The pass tests against the depth the scene drew, read-only with `LessEqual`, so each mesh lands on its own depth. Under `--msaa 4` it draws against the scene's samples and resolves, so its silhouettes are the picture's (the rgb then comes back times the coverage in alpha). A style reads it as `StyleFrame::albedo` by UV with `textureLoad`, and keys its bind groups by `frame.albedo_key()`.

What it leaves out keeps alpha 0, and a reader falls back on the picture there: terrain, grass, the ocean, billboards, particles, transparents, and draws with shallow wooze (#215), whose vertices move in a stage of their own. On an RTX 4070 at 1080p the pass costs 0.028–0.035 ms (p50 of `--gpu-time 240`, three alternating runs each on `stylize_props_plain` and `examples/ascii_dark_props`), and the pencil pass drew about 0.04 ms less, since it had less floor to hatch.

To read it in a style, set `uses_mask: MaskUse::Always` (or `MaskUse::When(f)` to read it only under some settings), make a layout with `kit::mask_layout` and bind it with `frame.mask_bind(...)`; in WGSL it is a `texture_2d<u32>` read with `textureLoad`, the id in `.r` and the blend in `.g` (`stylize/mask_view.rs` and `mask_view.wgsl` show the whole thing). A style needn't do anything with `.g`: the framework [cross-fades](#blend-for-free) its entities by it. One that does better than a cross-fade reads `.g` itself and sets `.blends_itself()`.

### Per-entity settings

A style that takes settings per entity names its component in its descriptor: `component: Some("ascii_style")`, with a schema in `schemas/components/`. The framework then does the rest:

- an entity carrying the component (or a `style_mask` naming the style) switches the style on for the frame, and the mask is drawn, with no `[post_process.styles]` table;
- the component's fields are read as the descriptor's params and options (a 3-array `x` fills `x_r`, `x_g`, `x_b`), and each distinct set is a `StyleVariant` with its own mask id, named for its settings (`ascii{cell=12,ramp=blocks}`, `ascii@barrel`, `ascii{off}` for `enabled = false`);
- a style whose `grid` option is `object` gets one variant per styled root, with `anchor` set to the root's world origin each frame (`frame.project(anchor)` puts it on screen).

In `encode`, `frame.variants()` lists this style's `(id, variant)` pairs, `frame.variant_param(v, "cell")`, `frame.variant_option(v, "ramp")` and `frame.variant_color(v, "tint")` read a variant's settings with the scene's as fallback, and `frame.whole_frame()` says whether the scene's table covers everything. `stylize/ascii.rs` and `stylize/pencil.rs` pack them into a table indexed by mask id; watercolor (#272) can do the same.

### Drawn only where the entities are

A per-entity style changes its entities' pixels and passes every other pixel through, so running its passes over the whole frame spends most of the GPU on pixels it gives back unchanged (#385). The framework knows where the entities can be: each frame it projects the world bounds of exactly the draws the mask draws for each id (terrain chunks in view, models, procedural boxes, and skinned meshes as posed, their bind-pose box under every bone matrix) and hands them to the style as `frame.styled_rect(pad)`. That is the union of the style's own variants' rects in pixels, grown by `pad` for how far a styled pixel reaches and by a pixel or two for the mask's rounding (its texels are larger under a render scale). It is `None` when the style covers the frame (`frame.covers_frame()`: its table on and `scope` not `entities`), when a draw has no bounds or crosses the camera plane, or under `FLINT_STYLE_SCISSOR=0`; an empty rect means none of its entities is in view.

Two helpers in `stylize::kit` use it:

- `kit::scissored_pass` for the pass that writes the style's output: the style draws inside the rect, and a blit (`kit::Blit`, made in `create` over the style's own input layout) copies the input into the up to four bands around it, so every pixel is written once. The blit reads with `textureLoad` at the pixel, as the styles' passthrough does, so the frame is the full pass's byte for byte, provided the style returns its input outside the rect.
- `kit::fullscreen_pass_in` for an intermediate pass whose result is read only near the entities: it draws inside the rect, and the rest stays cleared.

Pencil scissors its draw pass to the rect and its prep to the rect grown by how far a styled pixel reads the prep (the flow blur's `1.5 * spacing`, or a contour's wobble plus half its width). ASCII scissors its glyph pass to the rect grown by two of the largest cell in play (a snapped silhouette draws a whole cell around a styled pixel), its edge cell pass to the cells under that and one more on each side (thinning asks a neighbour), and its edge pixel pass, the 5x5 unprojection that is most of its cost, to those cells' pixels. On `examples/stylize_props.scene.toml` at 1080p (RTX 4070 laptop, best of five, alternating binaries) ASCII went from 0.42 to 0.22 ms and pencil from 0.072 to 0.048 ms; on `examples/ascii_entities.scene.toml` ASCII went from 0.45 to 0.17 ms. A whole-frame style isn't scissored, and the control case (`--style ascii` over the same props) was unchanged.

`SceneRenderer::set_style_scissor(false)` (or `FLINT_STYLE_SCISSOR=0`) draws the full passes, for checking: `per_entity_styles_scissored_to_their_props_draw_the_same_bytes` in `tests/stylize.rs` holds the two frames equal. A new per-entity style that scissors must keep its passthrough exact outside the rect, and must say how far a styled pixel reads in the pad.
