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
| `--width <px>` | 1920 | Image width in pixels (640 per column in a sweep, 960 in a GIF) |
| `--height <px>` | 1080 | Image height in pixels (480 per column in a sweep, 540 in a GIF) |
| `--distance <units>` | scene `[camera]` or auto | Camera distance from the target |
| `--yaw <degrees>` | scene `[camera]` or auto | Horizontal camera angle |
| `--pitch <degrees>` | scene `[camera]` or auto | Vertical camera angle |
| `--target <x,y,z>` | scene `[camera]` or auto | Camera look-at point (comma-separated) |
| `--fov <degrees>` | scene `[camera]` or auto | Field of view in degrees |
| `--no-grid` | `false` | Disable ground grid |
| `--schemas <path>` | `schemas` | Path to schemas directory (repeatable) |
| `--msaa <1\|4>` | `1` | Scene-pass MSAA sample count |

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
| `--kuwahara-boil <f32>` | `0.0` | How far each drawing moves the paint |
| `--contour` | off | Enable the contour pass |
| `--contour-width <f32>` | `1.25` | Half a line's width at 1080p (enables contour) |
| `--contour-threshold <f32>` | `0.08` | How strong an edge must be before it inks (enables contour) |
| `--contour-strength <f32>` | `0.9` | How dark the ink goes, 0--1 (enables contour) |
| `--contour-luma <f32>` | `0.5` | How much luminance edges count (enables contour) |
| `--contour-color <R,G,B>` | near black | The ink's colour, linear (enables contour) |
| `--contour-fade <f32>` | `30` | Metres at which a line has faded by half (enables contour) |
| `--contour-boil <f32>` | `0.0` | How far each drawing moves the lines (enables contour) |
| `--boil-on <u32>` | `2` | Film frames each drawing is held: 1, 2 or 3 |
| `--boil-time <f64>` | `0` | Seconds of the wooze field to run, which picks the drawing |
| `--render-mode <0-5>` | `0` | Stylized render mode |
| `--mode-mix <f32>` | `0` | Render mode blend, 0--1 |
| `--mode-params <x,y,z,w>` | `0,0,0,0` | Per-mode parameters |
| `--film-grain <f32>` | `0` | Film grain intensity |
| `--grain-time <f32>` | `0` | Post time driving grain and mode animation |
| `--grade-lift <r,g,b>` | `0,0,0` | Color grade lift |
| `--grade-gamma <r,g,b>` | `1,1,1` | Color grade gamma |
| `--grade-gain <r,g,b>` | `1,1,1` | Color grade gain |
| `--fxaa` | `false` | Run the FXAA pass |

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

# Other modes: depth, uv, unlit, metalrough
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
| `--debug-mode <mode>` | (none) | `wireframe`, `normals`, `depth`, `uv`, `unlit`, `metalrough` |
| `--wireframe-overlay` | `false` | Draw wireframe edges over solid shading |
| `--show-normals` | `false` | Draw face-normal direction arrows |
| `--no-tonemapping` | `false` | Disable tonemapping for raw linear output |

Both wireframe modes include skinned meshes, drawn in their bind pose.

## Determinism and the Gates

`flint render` is built to be byte-stable so that pixel-diff gates can trust it. Identical scene, flags and GPU driver produce identical PNGs, with these caveats:

- **Time is pinned.** Film grain and render-mode animation read a post time that headless render fixes at 0. Pass `--grain-time` to pick a different but equally repeatable frame; two renders at the same value match, different values differ.
- **MSAA and FXAA default off** precisely so the default path stays single-sample and single-pass. Turn them on for hero shots, not for baselines.
- **Animation does not run.** Skinned meshes render at bind pose. For a posed headless frame use the model previewer's render mode instead: `flint edit model.glb --render out.png --anim-time 1.5`, or `--sequence file.sequence.toml --anim-time 2.0` to replay a timed sequence deterministically, or `--pose-bone joint:x,y,z` to turn a joint by hand.
- **Particles start at t = 0**, so emitters appear empty unless you pass `--particle-time <s>`, which steps them deterministically before the capture. `--time` does this too.
- **Cost levers are free to change** without affecting geometry: `--ssao-samples 16` is roughly four times cheaper than the default 64 and usually indistinguishable on matte scenes (ADR 0052), which matters when a CI job renders dozens of frames.

## A Chosen Moment

`--time` captures a scene at a game time, so that two settings can be compared at the same instant:

```bash
flint render demo/living_props.scene.toml --time 3.5 -o at-3.5.png
```

[Living props](../concepts/procgen.md#in-a-scene-the-living_prop-component), shallow wooze, the [shading boil](../concepts/post-processing.md) and the [wooze field](../concepts/procgen.md#the-wooze-field) they all read run from zero to that time, and so do the other clocks the player keeps: grass, ocean and post time (`--grain-time` overrides the post time), particles (`--particle-time` overrides those), and `--boil-time` holds the boil on one drawing. The grid stays off, as it is in the player. The same value gives a byte-identical PNG on the same machine and driver.

It also matches the player. `flint render --time T` and `flint-player --frame-step S --screenshot-at T` at the same window size drew the same pixels on both of Flint's living demo scenes, at four times from 0 to 11.75 s and at steps of 1/16 and 1/60 s: not one pixel of 921,600 differed. Clocks step 1/60 s at a time, or `--frame-step S` if you want to name a player run's step. Scenes whose field doesn't change over time get the same pixels from any step.

Scripts, physics and animation clips don't run, and that's where the match ends. `demo/wooze_field.scene.toml` has a script that changes the step at 12 s, so a capture agrees with the player up to there and not after (about 50,000 pixels differ at 13 s). When a scene has scripts or `animator` components, the render names those entities in a warning. In `demo/shallow_wooze.scene.toml`, whose figure is on an idle clip, everything but the figure matches.

A scene with `living_prop` entities draws them in any `flint render`, at time 0 without `--time`. Procgen assets resolve as in the player only in those scenes. Other scenes draw exactly as they did.

## Sweeping a Knob

`--sweep` renders the same moment once per value of a wooze knob, side by side on one image:

```bash
flint render demo/living_props.scene.toml --time 3.5 --sweep wooze.glide=0,0.25,0.5,1 -o glide.png
```

![The same moment in the living oak bench at four values of glide, as labelled columns on a strip of film](../images/render-sweep-glide.png)

It takes `wooze.amplitude`, `wooze.step`, `wooze.glide`, `wooze.jitter` and `wooze.preset` (`wooze.preset=boil,morph,calm`). Each column is a whole run of its own from time 0, since a prop's state at a moment depends on what the knob was all the way there. `--width` and `--height` are each column's size, 640 x 480 by default.

A sweep sets the scene's global value, the same one a `[wooze]` table would, and a scene without a table gets an empty one. Zones still apply on top. If a zone keeps any prop off the value, the column says what the props really read (`PROPS READ 0.25-0.80`). If two neighbouring columns come out pixel-identical, a warning says so: at this moment, nothing on screen is listening to that knob.

## A Span of Time as a GIF

Stills can't show motion. Give `--time` a span and `--output` a `.gif`:

```bash
flint render demo/living_props.scene.toml --time 0..4 --fps 12 -o grove.gif
flint render demo/living_props.scene.toml --time 0..4 --sweep wooze.glide=0,1 -o glide.gif
flint gen specs/examples/oak_living.procgen.toml --strip --frames 48 --fps 24 -o oak.gif
```

The span leaves out its last moment, so that a span one cycle long loops cleanly. `--fps` goes up to 50, which is as fast as browsers will honestly play a GIF. A frame is 960 x 540 by default.

Nothing moves in the file that didn't move in the render. All the frames share one palette with no dithering, and a pixel the render left alone is written as transparent over the last frame. On 24 frames of the bench, in which 75.5% of the pixels never change, this keeps all of them still, where the `image` crate's encoder, with a new palette every frame, shimmers 81% of them by as much as 22 levels. That shimmer would look just like the boil you're trying to judge. It also comes out smaller: 1.8 MB to its 2.5 MB.

## What a Prop Was Doing

`--report json` writes one JSON object on stdout, and moves everything else to stderr:

```bash
flint render demo/wooze_field.scene.toml --time 3.5 -o at-3.5.png --report json
```

```json
{
  "scene": "The Wooze Field", "output": "at-3.5.png", "width": 1920, "height": 1080,
  "frame_step": 0.016666666666666666, "wooze_field": true, "wooze_zones": 2,
  "captures": [{
    "time": 3.5,
    "props": [{
      "entity": "oak_00", "asset": "oak_living@7", "lineage_seed": 7,
      "position": [-48.0, 0.0, -30.0], "phase_offset": 0.61965674, "beats": 7.619656741619093,
      "poses": { "index": 7, "weight": 0.13669996, "shape_seed": 11430650338220828881,
                 "lowest": 7, "highest": 8, "gliding_parts": 51, "parts": 243 },
      "wooze": { "amplitude": 1.0, "step": 0.5, "glide": 0.3, "jitter": 1.0 },
      "echoes": 6,
      "species": "oak_living",
      "values": {
        "step": { "value": 0.5, "layer": "global" },
        "glide": { "value": 0.3, "layer": "global" },
        "echoes.count": { "value": 6, "layer": "spec" },
        "stagger": { "value": "per_level", "layer": "spec" },
        "...": "amplitude, jitter, echoes.spread, echoes.alpha and variance.* the same way"
      }
    }]
  }],
  "still_props": [],
  "warnings": ["scripts don't run here, so what director would do to this scene isn't in it"]
}
```

There's one capture per image in the file: per column in a sweep (each with a `column`, the knob's value), and per frame in a span. Props come in order of name.

`poses` is what was drawn, read from the blend: `index` and `weight` are the first part of the skeleton, the trunk's first segment, leaving that pose toward the next, and `shape_seed` regrows it. `lowest` and `highest` are how far a staggered tree's parts have spread, and `gliding_parts` is how many are between poses instead of holding, so a frozen-looking tree has a number that says why. `wooze` is what the field said where the prop stands. `species` is the prop's spec, and `values` has every value a [species](../concepts/procgen.md#species-values) can set, as the prop runs on it, with the layer that set it: `spec`, `entity` (a `living_prop` stagger), `global` (the scene's `[living]` or `[wooze]`), `zone:<name>` where a zone has the last word, or `species`. A zone's `value` can be part way to its own, inside its falloff. `still_props` lists entities that asked to be living and drew as still models; the reason is in stderr.

Why the runtime values are here and not in `flint query`: query reads scene files, and none of this is in one.

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

## Visual Regression Testing

A basic visual regression workflow:

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
- **The full post-processing chain**, including Kuwahara, contour and FXAA when enabled

The only difference from interactive rendering is that the output goes to a texture-to-buffer copy instead of a swapchain surface.

## Further Reading

- [Rendering](../concepts/rendering.md) --- the PBR rendering pipeline
- [Post-Processing](../concepts/post-processing.md) --- what every flag above controls
- [Lighting](../concepts/lighting.md) --- the light component and shading levers
- [AI Agent Workflow](ai-agent-workflow.md) --- using headless renders for agent verification
- [CLI Reference](../cli-reference/overview.md) --- full command options
