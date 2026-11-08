# Headless Rendering

Flint can render scenes to PNG images without opening a window. This enables automated screenshots, visual regression testing, and CI pipeline integration.

## The `flint render` Command

```bash
flint render levels/tavern.scene.toml --output preview.png
```

This loads the scene, renders a single frame with PBR shading and shadows, applies the scene's `[post_process]` block, and writes the result to a PNG file.

## Camera Options

If the scene has a `[camera]` block, the render starts from that authored framing. The orbit-style flags override it:

```bash
flint render levels/tavern.scene.toml \
    --output preview.png \
    --width 1920 --height 1080 \
    --distance 30 \
    --yaw 45 \
    --pitch 30
```

| Flag | Default | Description |
|------|---------|-------------|
| `--output <path>` | `render.png` | Output file path |
| `--width <px>` | 1920 | Image width in pixels |
| `--height <px>` | 1080 | Image height in pixels |
| `--distance <units>` | scene `[camera]` or auto | Camera distance from the target |
| `--yaw <degrees>` | scene `[camera]` or auto | Horizontal camera angle |
| `--pitch <degrees>` | scene `[camera]` or auto | Vertical camera angle |
| `--target <x,y,z>` | scene `[camera]` or auto | Camera look-at point (comma-separated) |
| `--fov <degrees>` | scene `[camera]` or auto | Field of view in degrees |
| `--no-grid` | `false` | Disable ground grid |
| `--schemas <path>` | `schemas` | Path to schemas directory (repeatable) |
| `--msaa <1\|4>` | `1` | Scene-pass MSAA sample count |
| `--render-scale <f32>` | `1` | Draw the scene at this fraction of the image (0.25--1), upscaled |
| `--gpu-time <frames>` | (none) | Render this many frames with GPU timing and print each pass's ms |
| `--frames <n>` | (none) | Draw a moving sequence of `n` frames ([Frame sequences](#frame-sequences)) |

## Post-Processing Flags

Every `[post_process]` key has a flag. CLI values win over the scene block:

```bash
# Disable all post-processing (raw shader output)
flint render scene.toml --no-postprocess --output raw.png

# Custom bloom settings
flint render scene.toml --bloom-intensity 0.08 --bloom-threshold 0.8

# Adjust exposure
flint render scene.toml --exposure 1.5

# Cheaper SSAO, depth of field, a warm grade
flint render scene.toml --ssao-samples 16 --dof 0.6 --dof-focus 10 --dof-range 5 \
    --grade-lift 0.03,0.02,0.015 --grade-gain 1.04,1,0.94
```

| Flag | Default | Description |
|------|---------|-------------|
| `--no-postprocess` | `false` | Disable entire post-processing pipeline |
| `--bloom-intensity <f32>` | `0.04` | Bloom mix strength |
| `--bloom-threshold <f32>` | `1.0` | Minimum brightness for bloom |
| `--exposure <f32>` | `1.0` | Exposure multiplier |
| `--ssao-radius <f32>` | `0.5` | SSAO sample radius |
| `--ssao-intensity <f32>` | `1.0` | SSAO strength (0 disables) |
| `--ssao-samples <u32>` | `64` | SSAO samples per pixel, 1--64 |
| `--fog-density <f32>` | `0.02` | Fog density (enables fog; 0 disables) |
| `--fog-color <r,g,b>` | `0.7,0.75,0.82` | Fog color |
| `--fog-height-falloff <f32>` | `0.1` | Enables height fog with this falloff |
| `--volumetric-density <f32>` | `1.0` | Enables god rays with this density |
| `--volumetric-samples <u32>` | `32` | Volumetric ray-march steps |
| `--dither-intensity <f32>` | `0.03` | Enables ordered dither |
| `--desaturate <f32>` | `0` | Desaturation toward ash grey, 0--1 |
| `--dof <f32>` | `0` | Depth-of-field strength |
| `--dof-focus <f32>` | `10.0` | Focus plane distance, world units |
| `--dof-range <f32>` | `5.0` | Focus half-width, world units |
| `--kuwahara-radius <u32>` | `4` | Enables the Kuwahara filter with this radius |
| `--kuwahara-sharpness <f32>` | `8.0` | Kuwahara sector sharpness |
| `--kuwahara-hardness <f32>` | `8.0` | Kuwahara sector hardness |
| `--kuwahara-anisotropy <f32>` | `1.0` | Kuwahara anisotropy, 0--1 |
| `--edge-depth <f32>` / `--edge-normal <f32>` | `0.6` / `2.5` | Shared geometry edges' thresholds (`--style edges_view` shows what they find) |
| `--render-mode <0-5>` | `0` | Stylized render mode |
| `--mode-mix <f32>` | `0` | Render mode blend, 0--1 |
| `--mode-params <x,y,z,w>` | `0,0,0,0` | Per-mode parameters |
| `--film-grain <f32>` | `0` | Film grain intensity |
| `--grain-time <f32>` | `0` | Post time driving grain and mode animation |
| `--style <name[:k=v,...]>` | (none) | Turn on any registered style over the scene, e.g. `ascii:cell=12,ramp=blocks`, `invert`, `kuwahara:radius=6`, `ascii:off`; repeatable; `--style help` lists them, `--list-styles [--json]` with stages and ranges ([Stylize Passes](../concepts/stylize-passes.md#using-a-style)) |
| `--style-time <f32>` | `0` | Post time animated styles read (the same clock as `--grain-time`) |
| `--look <name\|path>` | (none) | A preset `[post_process]` block merged over the scene's: `ascii_noir`, `matrix`, `ink_wash`, `graphite`, or a `styles/<name>.look.toml`; other flags apply over it; `--look help` lists them ([Looks and sweeps](../concepts/stylize-passes.md#looks-and-sweeps)) |
| `--sweep <key=v1,v2,...>` | (none) | One labelled contact sheet of the frame at each value: `ascii.cell=6,8,12`, `post.exposure=0.8,1.2`, `look=a,b`, or `wooze.glide=0,1` (Living Props builds only); cells default to 640x360, and pixel-identical neighbours are warned about |
| `--grade-lift <r,g,b>` | `0,0,0` | Color grade lift |
| `--grade-gamma <r,g,b>` | `1,1,1` | Color grade gamma |
| `--grade-gain <r,g,b>` | `1,1,1` | Color grade gain |
| `--fxaa` | `false` | Run the FXAA pass |
| `--ascii` | (off) | The full-screen ASCII style (#265): one glyph per cell on one grid |
| `--ascii-font` / `--ascii-ramp` / `--ascii-color` | `builtin` / the set's / `source` | ASCII style glyph set, ramp and colour mode (`source`, `tint`, `raw`, `mono`, `palette`) |
| `--ascii-palette` / `--ascii-ground` / `--ascii-glow` | `phosphor` / `dim` / `0` | ASCII style palette (`cga`, `cga16`, `amber`... or a `.palette.toml`), ground (`dim`, `black`, `inverse`) and glyph halo |
| `--ascii-tint <r,g,b>` / `--ascii-background <f32>` | phosphor green / `0.15` | ASCII style tint and how much cell colour fills behind glyphs |
| `--ascii-brightness` / `--ascii-contrast` / `--ascii-ink` | `0` / `1` / `1` | ASCII style luminance remap and ink brightness |
| `--ascii-dither` | `0` | ASCII style ordered dither, fixed to the cell grid (#277) |
| `--ascii-exposure` / `--ascii-normalize` | `0` / `0` | ASCII style exposure in stops, and per-style-id tone normalisation so dark props stay legible (#289) |
| `--ascii-text <file>` / `--ascii-text-ansi <file>` | (none) | Also write the ASCII style's character grid as UTF-8 text, or with 24-bit ANSI colours ([Reading a render as text](#reading-a-render-as-text), #317) |

### Reading a render as text

An ASCII render is a real character grid, so it can also be read rather than looked at. `--ascii-text` writes the grid the frame drew: one line per cell row, one character per cell, the same cells as the PNG (part cells at the borders included). A cell the picture passes through, under `scope = "entities"` or where a later style draws its own entity, is a space.

```bash
flint render demo/pbr_test.scene.toml --ascii --ascii-text frame.txt
#   Wrote 240x135 ASCII grid to frame.txt
flint render demo/pbr_test.scene.toml --ascii --ascii-text-ansi frame.ans && cat frame.ans
```

The text fits in a prompt, diffs line by line between two runs, and lets a test say "the box is `#`" without pixel tolerances. The ANSI file paints each glyph in its ink over its ground, for a 24-bit terminal. The grid comes from the GPU: a pass of one fragment a cell records which glyph the glyph pass draws at the cell's centre, and the frame copies it back. It runs only while capture is on (`SceneRenderer::set_style_text_capture` / `style_text`). A cell on a styled entity's silhouette holds that glyph plus the outline ink the entity draws (#288), so there the text names the centre's glyph.
| `--pixels-per-point <f32>` | `1` | High-DPI scale factor for styles sized in points |
| `--ascii-cell <n>` | `8` | ASCII cell width in points |
| `--ascii-levels <n>` / `--ascii-footprint <cells>` | `0` / `1` | ASCII tone bands and tone footprint |
| `--ascii-no-edges` | `false` | No line glyphs on edges |
| `--ascii-edges <f32>` | `1` | ASCII style edge glyph strength (0 off) |
| `--ascii-edge-threshold <f32>` / `--ascii-edge-max <f32>` | `0.5` / `3` | ASCII style: cell widths of edge that make a line glyph, and past which it's texture |
| `--ascii-edge-depth <f32>` / `--ascii-edge-normal <f32>` | `0.6` / `2.5` | Edge sensitivity |

## Lighting Flags

The `[environment]` shading levers can be overridden too; see [Lighting](../concepts/lighting.md).

| Flag | Default | Description |
|------|---------|-------------|
| `--oren-nayar <f32>` | scene or `0` | Lambert to Oren-Nayar diffuse blend, 0--1 |
| `--sheen-strength <f32>` | scene or `0` | Charlie-sheen rim strength (keep at or below about 0.3) |
| `--sheen-color <r,g,b>` | `1,1,1` | Sheen tint |
| `--no-shadows` | `false` | Disable shadow mapping (also disables volumetric light) |
| `--shadow-resolution <px>` | `2048` | Shadow map resolution per cascade |

## Debug Rendering Flags

Render debug visualizations for diagnostics:

```bash
# Wireframe view
flint render scene.toml --debug-mode wireframe --output wireframe.png

# Surface normals
flint render scene.toml --debug-mode normals --output normals.png

# Other modes: depth, uv, unlit, metalrough, style, edges
flint render scene.toml --debug-mode depth --output depth.png

# Wireframe overlay on solid geometry
flint render scene.toml --wireframe-overlay --output overlay.png

# Normal arrows
flint render scene.toml --show-normals --output arrows.png

# Raw linear output (no tonemapping)
flint render scene.toml --no-tonemapping --output linear.png
```

| Flag | Default | Description |
|------|---------|-------------|
| `--debug-mode <mode>` | (none) | `wireframe`, `normals`, `depth`, `uv`, `unlit`, `metalrough`, `style`, `edges` |
| `--wireframe-overlay` | `false` | Draw wireframe edges over solid shading |
| `--show-normals` | `false` | Draw face-normal direction arrows |
| `--no-tonemapping` | `false` | Disable tonemapping for raw linear output |

Both wireframe modes include skinned meshes, drawn in their bind pose.

## Determinism and the Gates

`flint render` is built to be byte-stable so that pixel-diff gates can trust it. Identical scene, flags and GPU driver produce identical PNGs, with these caveats:

- **Time is pinned.** Film grain, render-mode animation and animated styles read a post time that headless render fixes at 0. Pass `--style-time` (or `--grain-time`) to pick a different but equally repeatable frame; two renders at the same value match, different values differ.
- **MSAA and FXAA default off** precisely so the default path stays single-sample and single-pass. Turn them on for hero shots, not for baselines.
- **Animation does not run.** Skinned meshes render at bind pose. For a posed headless frame use the model previewer's render mode instead: `flint edit model.glb --render out.png --anim-time 1.5`, or `--sequence file.sequence.toml --anim-time 2.0` to replay a timed sequence deterministically.
- **Particles start at t = 0**, so emitters appear empty unless you pass `--particle-time <s>`, which steps them deterministically before the capture.
- **Cost levers are free to change** without affecting geometry: `--ssao-samples 16` is roughly four times cheaper than the default 64 and usually indistinguishable on matte scenes (ADR 0052), which matters when a CI job renders dozens of frames.

## Frame sequences

One render is one frame, so a style that remembers the frame before (the ASCII style's per-cell history, #277) never gets to use its memory. `--frames <n>` draws a sequence on one renderer instead, `--frame-dt` apart (default 1/60 s), moving the camera between frames. The styles see consecutive frames and their history runs as it does in the player, without a window, a script or a capture tool (#345):

```bash
flint render demo/ascii_motion.scene.toml --schemas schemas --no-grid \
    --width 1280 --height 720 --frames 181 --orbit 6.8755 --output out/seq.png
#   Sequence: 181 frames, 0.016667 s apart, orbiting 6.8755 deg/s
#   Rendered 181 1280x720 frames to out/seq_0000.png .. out/seq_0180.png
```

| Flag | Default | Description |
|------|---------|-------------|
| `--frames <n>` | (none) | Frames to draw, written as `<stem>_0000.png`... beside `--output` (with `--ascii-text`, numbered grids too) |
| `--frame-dt <s>` | `1/60` | Seconds between frames. Post time (`--style-time` is the start), grass wind, `--particle-time` particles and the ASCII history's smoothing (#342) advance by it |
| `--orbit <deg/s>` | (still camera) | Turn the camera about its target, from the scene's `[camera]` or the orbit flags |
| `--camera-path <file>` | (still camera) | Follow keys instead, position and target interpolated linearly |
| `--flip-stats` | `false` | Count the ASCII style's glyph flips and reversals, per frame and in sum |
| `--flip-json <file>` | (none) | Write the counts as JSON (per-frame arrays and the means) |
| `--no-images` | `false` | Skip the PNGs, for a measurement |

A camera path is a TOML list of keys, in seconds from the first frame:

```toml
[[key]]
time = 0.0
position = [3.2, 1.5, 8.5]
target = [0.6, 0.6, 4.2]

[[key]]
time = 3.0
position = [5.4, 1.5, 6.9]
target = [0.6, 0.6, 4.2]
```

### Measuring shimmer

`--flip-stats` reads the character grid the ASCII style drew each frame (the [text capture](#reading-a-render-as-text)), so it counts glyphs exactly, with no thresholds on pixels. It prints two numbers. **Flips** are the cells whose glyph changed since the frame before, and real motion flips glyphs too. **Reversals** are the cells that changed back to the glyph they had two frames ago (A, B, A). Motion rarely does that in two frames, but shimmer does little else, so reversals measure the shimmer itself. Its summary line reads like `tools/ascii_flips.py`'s, and the two agree cell for cell on the same frames.

The #277 bench, with the history on and off and with and without the edge glyphs (#267):

```bash
flint render demo/ascii_motion.scene.toml --schemas schemas --no-grid \
    --width 1280 --height 720 --frames 181 --orbit 6.8755 --no-images --flip-stats \
    --style ascii:temporal=off --style ascii:edges=0
#   Flips: 181 frames, 14400 cells: 321.1 flips/frame (2.23%), 57.8 reversals/frame (0.40%)
```

| `demo/ascii_motion`, 181 frames | flips/frame | reversals/frame |
|---|---|---|
| `temporal = "off"`, `edges = 0` | 321.1 (2.23%) | 57.8 (0.40%) |
| `temporal = "on"`, `edges = 0` | 105.1 (0.73%) | **0.0** |
| `temporal = "off"`, edges on (default) | 389.7 (2.71%) | 69.1 (0.48%) |
| `temporal = "on"`, edges on (default) | 227.7 (1.58%) | 30.3 (0.21%) |

The history removes the tone glyphs' shimmer completely, as the player capture in #277 found (0.1 reversals a frame). The edge glyphs, added after that measurement, are still picked fresh every frame, and they are what's left (#477). The scene's `[camera]` is the first pose of its orbit script, so `--orbit 6.8755` (the script's 0.12 rad/s) follows the same path as the player's capture.

A sequence is byte-identical run to run: the same flags give the same PNGs, which the player's `--frame-step` captures don't quite manage (#341). A debug build draws this bench at about a second a frame; `--no-images` and `--ssao-samples 16` are the cheap ways to make it quicker. A `render-suite` gate on a reversal budget is #478.

## CI Pipeline Integration

Headless rendering works on machines without a display. Use it in CI to catch visual regressions:

```yaml
# Example GitHub Actions step
- name: Render preview
  run: |
    cargo run --bin flint -- render levels/tavern.scene.toml \
      --output screenshots/tavern.png \
      --width 1920 --height 1080

- name: Upload screenshot
  uses: actions/upload-artifact@v4
  with:
    name: screenshots
    path: screenshots/
```

## Golden Suites

`flint render-suite` given a `*.goldens.toml` renders a list of cases as `flint render` would and compares each with a golden PNG (#271). It exits non-zero when any case moves past its tolerance, so it works as a gate in a script or a test:

```bash
flint render-suite crates/flint-cli/tests/golden/stylize.goldens.toml
#   PASS  ascii-full      0.00% past 48 (limit 2.00%), mean 0.00 (limit 3.00), max 0, 1316 ms
#   ...
flint render-suite my.goldens.toml --case tavern --out-dir out/goldens
flint render-suite my.goldens.toml --bless          # write the renders over the goldens
```

```toml
[suite]
name = "stylize"
root = "../../../.."            # cases run from here; scene paths are relative to it
width = 640                     # default 640 x 360
height = 360
args = ["--schemas", "schemas"] # before every case's own flags

[tolerance]                     # the defaults below; a case may override any
level = 48                      # a pixel differs past this many levels (largest channel, 0-255)
share = 0.02                    # fail when more than this share of pixels differ...
mean = 3.0                      # ...or the mean difference passes this

[[case]]
name = "ascii-full"
scene = "demo/pbr_test.scene.toml"
golden = "ascii_pbr_test.png"   # beside the manifest; default "<name>.png"
args = ["--style", "ascii"]     # any flint render flags
# width, height and tolerance = { ... } override the suite's
text = "ascii_pbr_test.txt"     # optional: the ASCII grid too, compared exactly (#317)
text_cells = 0                  # ...or with at most this many cells different
```

A case with `text` is also a structural golden. It renders with `--ascii-text`, and its character grid must match the checked-in one cell for cell, with the PNG kept as the visual check. When cells move, `<case>.text.diff` lands beside the render. It shows each row that moved, the golden's line over the render's, with a caret under each cell, so a drift can be read without opening an image:

```text
row 4: 2 cell(s)
  golden |-----------------------------=====++++++++++++=====-----------------------------|
  render |-----------------------------=====+=+=++++++++=====-----------------------------|
                                             ^ ^
```

That's `ascii-full` with `brightness=-0.004` added. The PNG check passes it, with 0.19% of pixels moved against a 2% limit. The text check fails it, because 28 cells in 8 rows changed glyph. A structural golden is much stricter than a pixel tolerance, so give a case `text_cells` if its glyphs sit on a tone boundary that another GPU may round the other way.

### The contact sheet

Every run writes `index.html` to the out dir (#329): one card a case with the golden, the render and the diff side by side, then a meter for each number against its limit (the tick is the limit), the case's scene and flags, and the text diff when its grid moved. Cases that failed come first, then the ones where any pixel moved, then the identical ones, each group in manifest order; the buttons at the top filter by group. **blink** flips a render with its golden in place, which shows a one-pixel shift that a side-by-side hides. The page reads its images beside itself (`<case>.png`, `<case>.diff.png`, and `golden/<case>.png`, a copy of the golden as it was before the run), so the out dir can be zipped or uploaded whole.

A `--bless` still compares before it writes. Its sheet shows each case's old golden as **was** and the diff as what the bless changes: review that page, not the folder.

### One process

Cases run in the `render-suite` process itself (#329). Each one's flags are parsed as `flint render` parses them and go to the same function, on one GPU device kept for the whole run. Every glTF is decoded once (`flint_import::memoize_imports`) and every texture's mip chain is built once (`flint_render::memoize_mip_chains`). Each case still builds its own renderer, so it shares only the device and those decoded bytes with the case before it. For the 41 stylize cases in a debug build, the run's CPU time went from about 120 s to 55 s. Summing each case's best time over several runs, the suite went from 115 s to 45 s, and a props-scene case from about 3.2 s to 1.1 s.

The cost is crash isolation. A case that panics is run again in its own `flint render` process (the device is dropped and made again for the next case), and its log is the case's message. `--isolate` spawns every case, as before #329.

| Flag | Description |
|------|-------------|
| `--bless` | Write each render (and each `text` grid) over its golden instead of comparing. The contact sheet shows what each replaced. Look at it before committing |
| `--case <name>` | Only this case, or every case whose name contains it; repeatable |
| `--out-dir <dir>` | Where renders, `<case>.diff.png` (the golden dimmed, amber where a pixel moved, red past the level; written when any pixel moved), `report.json` and the `index.html` contact sheet go; default a folder in the system temp dir |
| `--require-gpu` | Fail instead of skipping when there is no GPU adapter (also `FLINT_REQUIRE_GPU=1`) |
| `--bless-variant` | Write each render as this adapter's variant of its golden, `<stem>.<key>.png` |
| `--isolate` | Run each case as its own `flint render` process: slower, but a crash takes only its case down |

Without a GPU adapter every case is skipped and the suite passes, unless `--require-gpu` or `FLINT_REQUIRE_GPU=1` is given. Under `FLINT_REQUIRE_GPU`, an adapter named by `WGPU_ADAPTER_NAME` that doesn't exist is also a failure; without it, the suite falls back to the default adapter.

### Adapters and golden variants

Two adapters never render exactly alike. The difference is usually small, but a backend can be as far off as a real regression, so a golden can carry a **variant** per adapter beside it: `kuwahara.warp.png` next to `kuwahara.png`. The suite prints the adapter it found and its key: `warp` (WARP, the software DX12 adapter), `lavapipe` (Mesa's software Vulkan), or else the backend's name (`vulkan`, `dx12`, `metal`, `gl`). A case compares with its adapter's variant when there is one, and with the plain golden otherwise. `--bless` rewrites whichever of the two the case compared with, and `--bless-variant` creates the variant. Each case in `report.json` records `adapter`, the `golden` it compared with, and `variant`, the path where this adapter's variant would go.

On CI, the `GPU tests (WARP)` job (`.github/workflows/ci.yml`) runs the stylize GPU tests and the engine goldens on WARP, with `FLINT_REQUIRE_GPU=1` set. Its summary lists every case's margin, and its `stylize-goldens-warp` artifact holds the renders and diffs, with the contact sheet: open its `index.html`. To bless a WARP variant, copy the case's render from that artifact to its report's `variant` path. wgpu hides WARP on a machine that has a GPU, so WARP variants can't be rendered locally. Engine styles are pinned by `crates/flint-cli/tests/golden/stylize.goldens.toml` ([Stylize Passes](../concepts/stylize-passes.md#goldens)).

## Visual Regression Testing

For a game's own scenes, a golden suite as above is the shortest path. By hand, the same workflow is:

1. **Baseline** --- render a reference image and commit it:
   ```bash
   flint render levels/tavern.scene.toml --output tests/baseline/tavern.png
   ```

2. **Test** --- after changes, render again and compare:
   ```bash
   flint render levels/tavern.scene.toml --output tests/current/tavern.png
   # Compare with your preferred tool (ImageMagick, pixelmatch, etc.)
   ```

3. **Update** --- if the change is intentional, update the baseline:
   ```bash
   cp tests/current/tavern.png tests/baseline/tavern.png
   ```

Keep baselines on the default flags (no `--msaa`, no `--fxaa`, no `--grain-time`) so a lever you added for a hero shot never silently changes the gate.

## Rendering Multiple Views

Script multiple renders for different angles:

```bash
#!/bin/bash
SCENE="levels/tavern.scene.toml"

for angle in 0 90 180 270; do
    flint render "$SCENE" \
        --output "screenshots/view_${angle}.png" \
        --yaw $angle --pitch 25 --distance 25 \
        --width 1920 --height 1080
done
```

## Rendering Pipeline Details

Headless rendering uses the same wgpu PBR pipeline as the interactive viewer:

- **Cook-Torrance BRDF** with roughness/metallic workflow and the `[environment]` shading levers
- **Cascaded shadow mapping** for directional light shadows, with PCSS when the light has an `angular_size`
- **glTF mesh rendering** with full material support
- **Skinned mesh rendering** at bind pose (no animation is evaluated headlessly)
- **The full post-processing chain**, including Kuwahara and FXAA when enabled

The only difference from interactive rendering is that the output goes to a texture-to-buffer copy instead of a swapchain surface.

## Further Reading

- [Rendering](../concepts/rendering.md) --- the PBR rendering pipeline
- [Post-Processing](../concepts/post-processing.md) --- what every flag above controls
- [Lighting](../concepts/lighting.md) --- the light component and shading levers
- [AI Agent Workflow](ai-agent-workflow.md) --- using headless renders for agent verification
- [CLI Reference](../cli-reference/overview.md) --- full command options
