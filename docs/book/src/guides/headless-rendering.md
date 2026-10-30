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
- **Animation does not run.** Skinned meshes render at bind pose. For a posed headless frame use the model previewer's render mode instead: `flint edit model.glb --render out.png --anim-time 1.5`, or `--sequence file.sequence.toml --anim-time 2.0` to replay a timed sequence deterministically.
- **Particles start at t = 0**, so emitters appear empty unless you pass `--particle-time <s>`, which steps them deterministically before the capture.
- **Cost levers are free to change** without affecting geometry: `--ssao-samples 16` is roughly four times cheaper than the default 64 and usually indistinguishable on matte scenes (ADR 0052), which matters when a CI job renders dozens of frames.

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
- **The full post-processing chain**, including Kuwahara and FXAA when enabled

The only difference from interactive rendering is that the output goes to a texture-to-buffer copy instead of a swapchain surface.

## Further Reading

- [Rendering](../concepts/rendering.md) --- the PBR rendering pipeline
- [Post-Processing](../concepts/post-processing.md) --- what every flag above controls
- [Lighting](../concepts/lighting.md) --- the light component and shading levers
- [AI Agent Workflow](ai-agent-workflow.md) --- using headless renders for agent verification
- [CLI Reference](../cli-reference/overview.md) --- full command options
