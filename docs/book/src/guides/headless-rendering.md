# Headless Rendering

Flint can render scenes to PNG images without opening a window. This enables automated screenshots, visual regression testing, and CI pipeline integration.

## The `flint render` Command

```bash
flint render levels/tavern.scene.toml --output preview.png
```

This loads the scene, renders a single frame with PBR shading and shadows, and writes the result to a PNG file.

## Camera Options

Control the camera position with orbit-style parameters:

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
| `--distance <units>` | (auto) | Camera distance from origin |
| `--yaw <degrees>` | (auto) | Horizontal camera angle |
| `--pitch <degrees>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point (comma-separated) |
| `--fov <degrees>` | (auto) | Field of view in degrees |
| `--no-grid` | `false` | Disable ground grid |
| `--schemas <path>` | `schemas` | Path to schemas directory (repeatable) |

## Post-Processing Flags

Control post-processing from the command line:

```bash
# Disable all post-processing (raw shader output)
flint render scene.toml --no-postprocess --output raw.png

# Custom bloom settings
flint render scene.toml --bloom-intensity 0.08 --bloom-threshold 0.8

# Adjust exposure
flint render scene.toml --exposure 1.5
```

| Flag | Default | Description |
|------|---------|-------------|
| `--no-postprocess` | `false` | Disable entire post-processing pipeline |
| `--bloom-intensity <f32>` | `0.04` | Bloom mix strength |
| `--bloom-threshold <f32>` | `1.0` | Minimum brightness for bloom |
| `--exposure <f32>` | `1.0` | Exposure multiplier |

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
| `--no-shadows` | `false` | Disable shadow mapping |
| `--shadow-resolution <px>` | `1024` | Shadow map resolution per cascade |

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

Since Flint's renderer is deterministic for a given scene file and camera position, identical inputs produce identical outputs.

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

- **Cook-Torrance BRDF** with roughness/metallic workflow
- **Cascaded shadow mapping** for directional light shadows
- **glTF mesh rendering** with full material support
- **Skinned mesh rendering** with bone matrix upload (for skeletal meshes)

The only difference from interactive rendering is that the output goes to a texture-to-buffer copy instead of a swapchain surface.

## Further Reading

- [Rendering](../concepts/rendering.md) --- the PBR rendering pipeline
- [AI Agent Workflow](ai-agent-workflow.md) --- using headless renders for agent verification
- [CLI Reference](../cli-reference/overview.md) --- full command options
