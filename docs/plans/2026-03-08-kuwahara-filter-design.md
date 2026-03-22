# Kuwahara Filter Post-Processing Effect — Design

## Overview

Add an anisotropic Kuwahara filter as a configurable post-processing effect. The filter produces a painterly/oil-painting look by selecting low-variance color regions, with brush strokes that follow local edge directions via a structure tensor.

## Architecture

Three-pass pipeline running as a separate pre-pass (like SSAO/volumetric), writing to an intermediate texture that the composite shader reads instead of the raw HDR buffer.

### Pass 1: Structure Tensor
- Compute Sobel gradients of HDR buffer luminance
- Output tensor components `(Jx*Jx, Jx*Jy, Jy*Jy, 0.0)` to `tensor_view` (Rgba16Float, full resolution)
- 9 texture taps per pixel (3x3 Sobel neighborhood)

### Pass 2: Tensor Blur
- 5x5 Gaussian blur on the structure tensor texture
- NOT depth-aware — orientations should bleed smoothly for coherent strokes
- Output to `tensor_blur_view` (Rgba16Float, full resolution)
- 25 texture taps per pixel

### Pass 3: Anisotropic Kuwahara
- Per pixel:
  1. Sample blurred structure tensor, extract eigenvalues/eigenvectors (closed-form 2x2)
  2. Compute anisotropy `A = (λ1 - λ2) / (λ1 + λ2)` and orientation angle `φ`
  3. For each of 8 sectors (rotated by `φ`):
     - Sample HDR buffer within an elliptical region (eccentricity = anisotropy param * A)
     - Accumulate weighted mean and variance using polynomial weights
  4. Blend sector contributions: weight each sector's mean by `exp(-sharpness * variance)`
  5. Output blended color to `kuwahara_view` (Rgba16Float, full resolution)
- ~60-80 texture taps per pixel at radius=4

## Render Pass Order

1. Shadow pass
2. Main scene pass → `hdr_view`
3. **Structure tensor** → `tensor_view` (reads `hdr_view`)
4. **Tensor blur** → `tensor_blur_view` (reads `tensor_view`)
5. **Anisotropic Kuwahara** → `kuwahara_view` (reads `hdr_view` + `tensor_blur_view`)
6. SSAO pass (reads depth)
7. Volumetric pass (reads depth + shadows)
8. Bloom pass (reads raw `hdr_view` — NOT filtered, so thresholds work on actual brightness)
9. Composite pass (reads `kuwahara_view` instead of `hdr_view` when enabled)

## Configuration

Parameters added to `PostProcessConfig`:

| Parameter | Type | Default | Range | Description |
|---|---|---|---|---|
| `kuwahara_enabled` | `bool` | `false` | — | Master toggle (F12) |
| `kuwahara_radius` | `u32` | `4` | 1–16 | Kernel radius. Larger = more painterly, more expensive |
| `kuwahara_sharpness` | `f32` | `8.0` | 0.1–32 | Sector selection sharpness. Higher = harder color region edges |
| `kuwahara_hardness` | `f32` | `8.0` | 0.1–32 | Sector boundary polynomial falloff. Higher = crisper strokes |
| `kuwahara_anisotropy` | `f32` | `1.0` | 0.0–1.0 | Edge-following strength. 0 = isotropic, 1 = fully anisotropic |

Fixed design choices:
- 8 sectors (standard, diminishing returns beyond this)
- 5x5 Gaussian tensor blur (technical parameter, rarely needs tuning)

TOML format (`[post_process]` section):
```toml
kuwahara_enabled = true
kuwahara_radius = 4
kuwahara_sharpness = 8.0
kuwahara_hardness = 8.0
kuwahara_anisotropy = 1.0
```

## GPU Resources

3 new textures (all Rgba16Float, full resolution, render target + texture binding):
- `structure_tensor_texture` / `structure_tensor_view`
- `structure_tensor_blur_texture` / `structure_tensor_blur_view`
- `kuwahara_texture` / `kuwahara_view`

3 new pipelines + uniform buffers:
- `structure_tensor_pipeline` + `structure_tensor_uniform_buffer`
- `tensor_blur_pipeline` + `tensor_blur_uniform_buffer`
- `kuwahara_pipeline` + `kuwahara_uniform_buffer`

## Shader Files

- `kuwahara_tensor_shader.wgsl` — Sobel gradients → tensor components
- `kuwahara_tensor_blur_shader.wgsl` — 5x5 Gaussian blur on tensor
- `kuwahara_shader.wgsl` — Anisotropic Kuwahara with 8 polynomial-weighted sectors

## Integration Points

### Files to modify:
- `crates/flint-render/src/postprocess.rs` — Config, uniforms, pipelines, resource allocation, render functions
- `crates/flint-render/src/scene_renderer/render_passes.rs` — Orchestration in `render_postprocess()`
- `crates/flint-scene/src/format.rs` — `PostProcessDef` TOML fields
- `crates/flint-player/src/player_app/mod.rs` — F12 toggle
- `crates/flint-cli/src/main.rs` — CLI flags (`--kuwahara-radius`, etc.)

### New files:
- `crates/flint-render/src/kuwahara_tensor_shader.wgsl`
- `crates/flint-render/src/kuwahara_tensor_blur_shader.wgsl`
- `crates/flint-render/src/kuwahara_shader.wgsl`

## Composite Integration

When `kuwahara_enabled`, the composite pass bind group 1 receives `kuwahara_view` instead of `hdr_view`. No shader changes needed — the composite shader already reads "the scene texture" generically. When disabled, falls back to `hdr_view` (same pattern as SSAO/volumetric fallback textures).

## Performance

- Default settings (radius=4): ~60-80 taps/pixel in Kuwahara pass, comparable to SSAO
- Full resolution required (half-res would cause visible artifacts)
- Cost scales roughly with radius squared
- Adequate for 60fps on discrete GPUs; high radii may be expensive on integrated GPUs
