# Kuwahara Filter Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an anisotropic Kuwahara filter as a configurable post-processing effect that produces a painterly/oil-painting look with edge-following brush strokes.

**Architecture:** Three-pass pipeline (structure tensor → tensor blur → anisotropic Kuwahara) running before bloom/SSAO/volumetric. The composite shader reads the filtered output instead of raw HDR when enabled. Follows the exact same pattern as existing SSAO and volumetric effects.

**Tech Stack:** wgpu 23, WGSL shaders, bytemuck for uniform structs, clap for CLI args, serde for TOML config.

**Design doc:** `docs/plans/2026-03-08-kuwahara-filter-design.md`

---

### Task 1: Add Kuwahara config fields

Add the 5 Kuwahara parameters to the runtime config struct and its default impl.

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs:18-49` (PostProcessConfig struct)
- Modify: `crates/flint-render/src/postprocess.rs:51-86` (Default impl)

**Step 1: Add fields to PostProcessConfig**

Add these fields after `volumetric_decay` (line 48), before the closing brace:

```rust
    pub kuwahara_enabled: bool,
    pub kuwahara_radius: u32,
    pub kuwahara_sharpness: f32,
    pub kuwahara_hardness: f32,
    pub kuwahara_anisotropy: f32,
```

**Step 2: Add defaults**

Add these lines in `Default::default()` after `volumetric_decay: 0.98,` (line 83):

```rust
            kuwahara_enabled: false,
            kuwahara_radius: 4,
            kuwahara_sharpness: 8.0,
            kuwahara_hardness: 8.0,
            kuwahara_anisotropy: 1.0,
```

**Step 3: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build (no other code references these fields yet)

**Step 4: Commit**

```
git add crates/flint-render/src/postprocess.rs
git commit -m "Add Kuwahara filter config fields to PostProcessConfig"
```

---

### Task 2: Add Kuwahara TOML config fields

Add Kuwahara fields to the scene file format so they can be set in `[post_process]` TOML sections.

**Files:**
- Modify: `crates/flint-scene/src/format.rs:49-100` (PostProcessDef struct)
- Modify: `crates/flint-scene/src/format.rs` (add default functions after existing ones, ~line 170)

**Step 1: Add fields to PostProcessDef**

Add after `volumetric_decay` field (line 99), before the closing brace:

```rust
    #[serde(default)]
    pub kuwahara_enabled: bool,
    #[serde(default = "default_kuwahara_radius")]
    pub kuwahara_radius: u32,
    #[serde(default = "default_kuwahara_sharpness")]
    pub kuwahara_sharpness: f32,
    #[serde(default = "default_kuwahara_hardness")]
    pub kuwahara_hardness: f32,
    #[serde(default = "default_kuwahara_anisotropy")]
    pub kuwahara_anisotropy: f32,
```

**Step 2: Add default functions**

Add after `default_volumetric_decay()` (after line 172):

```rust
fn default_kuwahara_radius() -> u32 {
    4
}

fn default_kuwahara_sharpness() -> f32 {
    8.0
}

fn default_kuwahara_hardness() -> f32 {
    8.0
}

fn default_kuwahara_anisotropy() -> f32 {
    1.0
}
```

**Step 3: Add to `post_process_config_from_def`**

In `crates/flint-player/src/player_app/scene_loading.rs:24-51`, add after line 50 (`config.volumetric_decay = ...`):

```rust
    config.kuwahara_enabled = pp_def.kuwahara_enabled;
    config.kuwahara_radius = pp_def.kuwahara_radius;
    config.kuwahara_sharpness = pp_def.kuwahara_sharpness;
    config.kuwahara_hardness = pp_def.kuwahara_hardness;
    config.kuwahara_anisotropy = pp_def.kuwahara_anisotropy;
```

**Step 4: Verify it compiles**

Run: `cargo build -p flint-scene -p flint-player 2>&1 | head -5`
Expected: successful build

**Step 5: Commit**

```
git add crates/flint-scene/src/format.rs crates/flint-player/src/player_app/scene_loading.rs
git commit -m "Add Kuwahara filter fields to PostProcessDef TOML format"
```

---

### Task 3: Add Kuwahara uniform structs

Define the GPU-side uniform structs for all three Kuwahara passes.

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs` (add after `SsaoBlurUniforms`, ~line 176)

**Step 1: Add uniform structs**

Add after `SsaoBlurUniforms` (after line 176):

```rust
/// Uniform data for the structure tensor pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaTensorUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// Uniform data for the structure tensor blur pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaTensorBlurUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// Uniform data for the anisotropic Kuwahara filter pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaUniforms {
    pub texel_size: [f32; 2],
    pub radius: f32,
    pub sharpness: f32,
    pub hardness: f32,
    pub anisotropy: f32,
    pub _pad: [f32; 2],
}
```

**Step 2: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build

**Step 3: Commit**

```
git add crates/flint-render/src/postprocess.rs
git commit -m "Add Kuwahara uniform structs for GPU passes"
```

---

### Task 4: Write the structure tensor shader

Computes Sobel gradients of HDR luminance and outputs tensor components.

**Files:**
- Create: `crates/flint-render/src/kuwahara_tensor_shader.wgsl`

**Step 1: Write the shader**

```wgsl
// Structure Tensor computation for Anisotropic Kuwahara filter
//
// Computes Sobel gradients of the HDR buffer luminance and outputs
// the structure tensor components (Jx*Jx, Jx*Jy, Jy*Jy, 0) per pixel.

struct KuwaharaTensorUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaTensorUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Fullscreen triangle
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;

    // Sample 3x3 neighborhood luminance
    let tl = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, -tx.y)).rgb);
    let tc = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(0.0, -tx.y)).rgb);
    let tr = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, -tx.y)).rgb);
    let ml = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, 0.0)).rgb);
    let mr = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, 0.0)).rgb);
    let bl = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(-tx.x, tx.y)).rgb);
    let bc = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(0.0, tx.y)).rgb);
    let br = luminance(textureSample(hdr_texture, hdr_sampler, in.uv + vec2<f32>(tx.x, tx.y)).rgb);

    // Sobel gradients
    let jx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    let jy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);

    // Structure tensor components: (E, F, G, 0) = (Jx*Jx, Jx*Jy, Jy*Jy, 0)
    return vec4<f32>(jx * jx, jx * jy, jy * jy, 0.0);
}
```

**Step 2: Verify shader is valid WGSL syntax**

Run: `cargo build -p flint-render 2>&1 | head -5`
(Won't be used yet, but including it validates the file exists for later `include_str!`)

**Step 3: Commit**

```
git add crates/flint-render/src/kuwahara_tensor_shader.wgsl
git commit -m "Add structure tensor shader for Kuwahara filter"
```

---

### Task 5: Write the tensor blur shader

5x5 Gaussian blur on the structure tensor texture (not depth-aware).

**Files:**
- Create: `crates/flint-render/src/kuwahara_tensor_blur_shader.wgsl`

**Step 1: Write the shader**

```wgsl
// Structure Tensor Gaussian Blur for Anisotropic Kuwahara filter
//
// 5x5 Gaussian blur on the structure tensor texture.
// NOT depth-aware — orientations should bleed smoothly for coherent brush strokes.

struct KuwaharaTensorBlurUniforms {
    texel_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaTensorBlurUniforms;

@group(1) @binding(0)
var tensor_texture: texture_2d<f32>;
@group(1) @binding(1)
var tensor_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;

    // 5x5 Gaussian kernel (sigma ~1.0, normalized)
    // Weights: 1 4 6 4 1 / 256 (2D separable product)
    let w = array<f32, 5>(1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0);

    var result = vec4<f32>(0.0);
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
            let weight = w[x + 2] * w[y + 2];
            result += textureSample(tensor_texture, tensor_sampler, in.uv + offset) * weight;
        }
    }

    return result;
}
```

**Step 2: Commit**

```
git add crates/flint-render/src/kuwahara_tensor_blur_shader.wgsl
git commit -m "Add structure tensor blur shader for Kuwahara filter"
```

---

### Task 6: Write the anisotropic Kuwahara shader

The main filter: reads blurred structure tensor + HDR buffer, outputs painterly-filtered color.

**Files:**
- Create: `crates/flint-render/src/kuwahara_shader.wgsl`

**Step 1: Write the shader**

```wgsl
// Anisotropic Kuwahara Filter
//
// Uses the blurred structure tensor to orient 8 elliptical sectors along
// local edge directions. Each sector accumulates weighted mean and variance,
// then sectors are blended with low-variance sectors dominating.
// Based on Kyprianidis et al. "Anisotropic Kuwahara Filtering on the GPU"

struct KuwaharaUniforms {
    texel_size: vec2<f32>,
    radius: f32,
    sharpness: f32,
    hardness: f32,
    anisotropy: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> params: KuwaharaUniforms;

@group(1) @binding(0)
var hdr_texture: texture_2d<f32>;
@group(1) @binding(1)
var hdr_sampler: sampler;

@group(2) @binding(0)
var tensor_texture: texture_2d<f32>;
@group(2) @binding(1)
var tensor_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

const PI: f32 = 3.14159265359;
const NUM_SECTORS: u32 = 8u;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tx = params.texel_size;
    let radius = i32(params.radius);

    // Sample blurred structure tensor
    let tensor = textureSample(tensor_texture, tensor_sampler, in.uv);
    let E = tensor.r; // Jx*Jx
    let F = tensor.g; // Jx*Jy
    let G = tensor.b; // Jy*Jy

    // Eigenvalue decomposition of 2x2 symmetric matrix [[E,F],[F,G]]
    let disc = sqrt(max((E - G) * (E - G) + 4.0 * F * F, 0.0));
    let lambda1 = 0.5 * (E + G + disc);
    let lambda2 = 0.5 * (E + G - disc);

    // Orientation angle (direction of major eigenvector)
    let phi = 0.5 * atan2(2.0 * F, E - G);

    // Anisotropy measure [0,1]
    let A = (lambda1 - lambda2) / max(lambda1 + lambda2, 0.0001);
    let aniso = mix(1.0, 1.0 / max(1.0 - A * params.anisotropy, 0.1), params.anisotropy);

    // Rotation matrix for aligning sectors with edge direction
    let cos_phi = cos(phi);
    let sin_phi = sin(phi);

    // Per-sector accumulators
    var total_weight = 0.0;
    var total_color = vec3<f32>(0.0);

    let sector_angle = 2.0 * PI / f32(NUM_SECTORS);

    for (var s: u32 = 0u; s < NUM_SECTORS; s = s + 1u) {
        var mean = vec3<f32>(0.0);
        var mean_sq = vec3<f32>(0.0);
        var w_sum = 0.0;

        let sector_center_angle = f32(s) * sector_angle;

        for (var y: i32 = -radius; y <= radius; y = y + 1) {
            for (var x: i32 = -radius; x <= radius; x = x + 1) {
                // Rotate sample position by -phi to align with edge
                let fx = f32(x);
                let fy = f32(y);
                let rx = fx * cos_phi + fy * sin_phi;
                let ry = -fx * sin_phi + fy * cos_phi;

                // Apply anisotropic scaling (stretch along edge direction)
                let sx = rx;
                let sy = ry * aniso;

                // Check if sample is within the elliptical radius
                let dist = sqrt(sx * sx + sy * sy);
                if (dist > f32(radius)) {
                    continue;
                }

                // Determine which sector this sample falls in
                let sample_angle = atan2(sy, sx) + PI;
                var sector_idx = u32(floor(sample_angle / sector_angle)) % NUM_SECTORS;

                if (sector_idx != s) {
                    continue;
                }

                // Polynomial weight (Gaussian-like falloff from center)
                let norm_dist = dist / f32(radius);
                let w = pow(1.0 - norm_dist, params.hardness);

                let offset = vec2<f32>(f32(x) * tx.x, f32(y) * tx.y);
                let color = textureSample(hdr_texture, hdr_sampler, in.uv + offset).rgb;

                mean += color * w;
                mean_sq += color * color * w;
                w_sum += w;
            }
        }

        // Compute sector mean and variance
        if (w_sum > 0.0) {
            mean /= w_sum;
            mean_sq /= w_sum;
            let variance = dot(mean_sq - mean * mean, vec3<f32>(1.0 / 3.0));
            let sector_weight = exp(-params.sharpness * max(variance, 0.0));
            total_color += mean * sector_weight;
            total_weight += sector_weight;
        }
    }

    if (total_weight > 0.0) {
        total_color /= total_weight;
    } else {
        total_color = textureSample(hdr_texture, hdr_sampler, in.uv).rgb;
    }

    return vec4<f32>(total_color, 1.0);
}
```

**Step 2: Commit**

```
git add crates/flint-render/src/kuwahara_shader.wgsl
git commit -m "Add anisotropic Kuwahara filter shader"
```

---

### Task 7: Add Kuwahara textures to PostProcessResources

Allocate the 3 intermediate textures on creation and resize.

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs:183-201` (PostProcessResources struct)
- Modify: `crates/flint-render/src/postprocess.rs:1991-2135` (PostProcessResources::new)

**Step 1: Add fields to PostProcessResources**

Add after the volumetric fields (after line 200), before the closing brace:

```rust
    // Kuwahara textures (full resolution, Rgba16Float)
    pub kuwahara_tensor_texture: wgpu::Texture,
    pub kuwahara_tensor_view: wgpu::TextureView,
    pub kuwahara_tensor_blur_texture: wgpu::Texture,
    pub kuwahara_tensor_blur_view: wgpu::TextureView,
    pub kuwahara_texture: wgpu::Texture,
    pub kuwahara_view: wgpu::TextureView,
```

**Step 2: Allocate textures in `PostProcessResources::new()`**

Add before the `Self { ... }` return block (before line 2119). Place after the volumetric texture creation:

```rust
        // Kuwahara textures (full resolution, Rgba16Float)
        let kuwahara_tensor_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Tensor Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let kuwahara_tensor_view =
            kuwahara_tensor_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let kuwahara_tensor_blur_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Tensor Blur Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let kuwahara_tensor_blur_view =
            kuwahara_tensor_blur_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let kuwahara_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let kuwahara_view =
            kuwahara_texture.create_view(&wgpu::TextureViewDescriptor::default());
```

**Step 3: Add to the Self return block**

Add after `volumetric_blur_view,` in the `Self { ... }` block:

```rust
            kuwahara_tensor_texture,
            kuwahara_tensor_view,
            kuwahara_tensor_blur_texture,
            kuwahara_tensor_blur_view,
            kuwahara_texture,
            kuwahara_view,
```

**Step 4: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build

**Step 5: Commit**

```
git add crates/flint-render/src/postprocess.rs
git commit -m "Add Kuwahara textures to PostProcessResources"
```

---

### Task 8: Add Kuwahara pipelines to PostProcessPipeline

Create the 3 render pipelines, bind group layouts, and uniform buffers.

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs:212-261` (PostProcessPipeline struct fields)
- Modify: `crates/flint-render/src/postprocess.rs:265+` (PostProcessPipeline::new)

**Step 1: Add fields to PostProcessPipeline**

Add after `volumetric_blur_uniform_buffer` / `volumetric_black_view` (after line 260), before closing brace:

```rust
    // Kuwahara filter pipelines and resources
    pub kuwahara_tensor_pipeline: wgpu::RenderPipeline,
    pub kuwahara_tensor_uniform_bgl: wgpu::BindGroupLayout,
    pub kuwahara_tensor_texture_bgl: wgpu::BindGroupLayout,
    pub kuwahara_tensor_uniform_buffer: wgpu::Buffer,
    pub kuwahara_tensor_blur_pipeline: wgpu::RenderPipeline,
    pub kuwahara_tensor_blur_uniform_bgl: wgpu::BindGroupLayout,
    pub kuwahara_tensor_blur_texture_bgl: wgpu::BindGroupLayout,
    pub kuwahara_tensor_blur_uniform_buffer: wgpu::Buffer,
    pub kuwahara_pipeline: wgpu::RenderPipeline,
    pub kuwahara_uniform_bgl: wgpu::BindGroupLayout,
    pub kuwahara_hdr_bgl: wgpu::BindGroupLayout,
    pub kuwahara_tensor_input_bgl: wgpu::BindGroupLayout,
    pub kuwahara_uniform_buffer: wgpu::Buffer,
```

**Step 2: Create pipelines in `PostProcessPipeline::new()`**

Add the pipeline creation code at the end of `new()`, before the `Self { ... }` return block. Follow the exact pattern used for SSAO/volumetric pipelines (same bind group layout pattern: group 0 = uniforms, group 1 = source texture + sampler, group 2 = secondary texture + sampler where needed).

All three pipelines use HDR_FORMAT as the render target format. Each uses the standard fullscreen triangle vertex shader pattern (0..3 vertices, no vertex buffers). Use `include_str!("kuwahara_tensor_shader.wgsl")`, `include_str!("kuwahara_tensor_blur_shader.wgsl")`, and `include_str!("kuwahara_shader.wgsl")` respectively.

**Structure tensor pipeline:** 2 bind groups — group 0: tensor uniforms, group 1: HDR texture + linear sampler (filterable float texture + filtering sampler).

**Tensor blur pipeline:** 2 bind groups — group 0: blur uniforms, group 1: tensor texture + linear sampler.

**Kuwahara pipeline:** 3 bind groups — group 0: kuwahara uniforms, group 1: HDR texture + linear sampler, group 2: blurred tensor texture + linear sampler.

For all three, the render pipeline target format is `HDR_FORMAT` (Rgba16Float), no blend state, no depth.

Create uniform buffers using the same pattern as existing ones:
```rust
let kuwahara_tensor_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
    label: Some("Kuwahara Tensor Uniform Buffer"),
    size: std::mem::size_of::<KuwaharaTensorUniforms>() as u64,
    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    mapped_at_creation: false,
});
```

**Step 3: Add all new fields to the `Self { ... }` return block**

```rust
            kuwahara_tensor_pipeline,
            kuwahara_tensor_uniform_bgl,
            kuwahara_tensor_texture_bgl,
            kuwahara_tensor_uniform_buffer,
            kuwahara_tensor_blur_pipeline,
            kuwahara_tensor_blur_uniform_bgl,
            kuwahara_tensor_blur_texture_bgl,
            kuwahara_tensor_blur_uniform_buffer,
            kuwahara_pipeline,
            kuwahara_uniform_bgl,
            kuwahara_hdr_bgl,
            kuwahara_tensor_input_bgl,
            kuwahara_uniform_buffer,
```

**Step 4: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build

**Step 5: Commit**

```
git add crates/flint-render/src/postprocess.rs
git commit -m "Add Kuwahara pipeline creation to PostProcessPipeline"
```

---

### Task 9: Implement Kuwahara render functions

Add the `run_kuwahara()` method that executes all three passes.

**Files:**
- Modify: `crates/flint-render/src/postprocess.rs` (add new `impl` method before `composite()`, ~line 1807)

**Step 1: Add `run_kuwahara` method**

Add to `impl PostProcessPipeline`, before the `composite()` method:

```rust
    /// Run the anisotropic Kuwahara filter: structure tensor → blur → filter.
    pub fn run_kuwahara(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
    ) {
        let texel_size = [1.0 / resources.width as f32, 1.0 / resources.height as f32];

        // --- Pass 1: Structure tensor ---
        let tensor_uniforms = KuwaharaTensorUniforms {
            texel_size,
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.kuwahara_tensor_uniform_buffer,
            0,
            bytemuck::cast_slice(&[tensor_uniforms]),
        );

        let tensor_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Tensor Uniform BG"),
            layout: &self.kuwahara_tensor_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.kuwahara_tensor_uniform_buffer.as_entire_binding(),
            }],
        });

        let tensor_hdr_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Tensor HDR BG"),
            layout: &self.kuwahara_tensor_texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Kuwahara Tensor Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kuwahara Tensor Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.kuwahara_tensor_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.kuwahara_tensor_pipeline);
            pass.set_bind_group(0, &tensor_uniform_bg, &[]);
            pass.set_bind_group(1, &tensor_hdr_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // --- Pass 2: Tensor blur ---
        let blur_uniforms = KuwaharaTensorBlurUniforms {
            texel_size,
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.kuwahara_tensor_blur_uniform_buffer,
            0,
            bytemuck::cast_slice(&[blur_uniforms]),
        );

        let blur_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Tensor Blur Uniform BG"),
            layout: &self.kuwahara_tensor_blur_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.kuwahara_tensor_blur_uniform_buffer.as_entire_binding(),
            }],
        });

        let blur_tensor_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Tensor Blur Input BG"),
            layout: &self.kuwahara_tensor_blur_texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.kuwahara_tensor_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Kuwahara Tensor Blur Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kuwahara Tensor Blur Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.kuwahara_tensor_blur_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.kuwahara_tensor_blur_pipeline);
            pass.set_bind_group(0, &blur_uniform_bg, &[]);
            pass.set_bind_group(1, &blur_tensor_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // --- Pass 3: Anisotropic Kuwahara filter ---
        let kuwahara_uniforms = KuwaharaUniforms {
            texel_size,
            radius: config.kuwahara_radius as f32,
            sharpness: config.kuwahara_sharpness,
            hardness: config.kuwahara_hardness,
            anisotropy: config.kuwahara_anisotropy,
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.kuwahara_uniform_buffer,
            0,
            bytemuck::cast_slice(&[kuwahara_uniforms]),
        );

        let kuwahara_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Uniform BG"),
            layout: &self.kuwahara_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.kuwahara_uniform_buffer.as_entire_binding(),
            }],
        });

        let kuwahara_hdr_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara HDR BG"),
            layout: &self.kuwahara_hdr_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let kuwahara_tensor_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kuwahara Tensor Input BG"),
            layout: &self.kuwahara_tensor_input_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &resources.kuwahara_tensor_blur_view,
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Kuwahara Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kuwahara Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.kuwahara_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.kuwahara_pipeline);
            pass.set_bind_group(0, &kuwahara_uniform_bg, &[]);
            pass.set_bind_group(1, &kuwahara_hdr_bg, &[]);
            pass.set_bind_group(2, &kuwahara_tensor_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
```

**Step 2: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build

**Step 3: Commit**

```
git add crates/flint-render/src/postprocess.rs
git commit -m "Implement Kuwahara render functions (3-pass pipeline)"
```

---

### Task 10: Integrate Kuwahara into composite and render pass orchestration

Wire up the Kuwahara passes into the render loop and modify composite to use the filtered output.

**Files:**
- Modify: `crates/flint-render/src/scene_renderer/render_passes.rs:890-958` (render_postprocess)
- Modify: `crates/flint-render/src/postprocess.rs` (composite method, ~line 1920-1932, scene bind group)

**Step 1: Add Kuwahara pass to render_postprocess**

In `render_postprocess()`, add after the scene render / before SSAO (after line 906, before "Run SSAO if enabled"):

```rust
        // Run Kuwahara filter if enabled
        if self.postprocess_config.enabled && self.postprocess_config.kuwahara_enabled {
            pp.run_kuwahara(device, queue, resources, &self.postprocess_config);
        }
```

**Step 2: Modify composite to use kuwahara_view**

In `composite()`, change the scene bind group (around lines 1920-1932) to conditionally use `kuwahara_view`:

Replace:
```rust
        let scene_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Scene BG"),
            layout: &self.composite_scene_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.hdr_view),
                },
```

With:
```rust
        // Use Kuwahara-filtered output if enabled, raw HDR otherwise
        let scene_view = if config.enabled && config.kuwahara_enabled {
            &resources.kuwahara_view
        } else {
            &resources.hdr_view
        };

        let scene_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Scene BG"),
            layout: &self.composite_scene_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene_view),
                },
```

**Step 3: Verify it compiles**

Run: `cargo build -p flint-render 2>&1 | head -5`
Expected: successful build

**Step 4: Commit**

```
git add crates/flint-render/src/postprocess.rs crates/flint-render/src/scene_renderer/render_passes.rs
git commit -m "Integrate Kuwahara filter into render pass orchestration and composite"
```

---

### Task 11: Add F12 toggle and CLI flags

Wire up runtime toggle and CLI configuration.

**Files:**
- Modify: `crates/flint-player/src/player_app/mod.rs` (~line 1973, after F11 handler)
- Modify: `crates/flint-cli/src/commands/render.rs:14-44` (RenderArgs struct)
- Modify: `crates/flint-cli/src/commands/render.rs` (~line 239, after volumetric_samples handling)
- Modify: `crates/flint-cli/src/main.rs` (CLI arg definitions and forwarding)

**Step 1: Add F12 handler in player_app**

After the F11 fullscreen handler block (after the `}` closing the F11 arm), add:

```rust
                                KeyCode::F12 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.kuwahara_enabled = !config.kuwahara_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
```

**Step 2: Add CLI args to RenderArgs**

Add after `volumetric_samples` in the `RenderArgs` struct:

```rust
    pub kuwahara_radius: Option<u32>,
    pub kuwahara_sharpness: Option<f32>,
    pub kuwahara_hardness: Option<f32>,
    pub kuwahara_anisotropy: Option<f32>,
```

**Step 3: Add CLI arg handling in render.rs**

Add after the `volumetric_samples` handling block:

```rust
        if let Some(radius) = args.kuwahara_radius {
            pp_config.kuwahara_radius = radius;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(sharpness) = args.kuwahara_sharpness {
            pp_config.kuwahara_sharpness = sharpness;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(hardness) = args.kuwahara_hardness {
            pp_config.kuwahara_hardness = hardness;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(anisotropy) = args.kuwahara_anisotropy {
            pp_config.kuwahara_anisotropy = anisotropy;
            pp_config.kuwahara_enabled = true;
        }
```

**Step 4: Add clap arg definitions in main.rs**

Find the CLI render command definition. Add after the `volumetric_samples` arg:

```rust
        /// Kuwahara filter radius (enables Kuwahara; default: 4)
        #[arg(long)]
        kuwahara_radius: Option<u32>,
        /// Kuwahara sector sharpness (default: 8.0)
        #[arg(long)]
        kuwahara_sharpness: Option<f32>,
        /// Kuwahara sector hardness (default: 8.0)
        #[arg(long)]
        kuwahara_hardness: Option<f32>,
        /// Kuwahara anisotropy strength (0=isotropic, 1=full; default: 1.0)
        #[arg(long)]
        kuwahara_anisotropy: Option<f32>,
```

Also add the forwarding in the match arm that constructs `RenderArgs`:

```rust
            kuwahara_radius,
            kuwahara_sharpness,
            kuwahara_hardness,
            kuwahara_anisotropy,
```

**Step 5: Verify full build**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

**Step 6: Commit**

```
git add crates/flint-player/src/player_app/mod.rs crates/flint-cli/src/commands/render.rs crates/flint-cli/src/main.rs
git commit -m "Add F12 toggle and CLI flags for Kuwahara filter"
```

---

### Task 12: Test with flint render

Verify the effect works end-to-end with a real scene.

**Files:** None (testing only)

**Step 1: Render without Kuwahara (baseline)**

Run: `cargo run --bin flint -- render demo/phase4_runtime.scene.toml --output /tmp/no_kuwahara.png --schemas schemas --width 1280 --height 720`

**Step 2: Render with Kuwahara enabled**

Run: `cargo run --bin flint -- render demo/phase4_runtime.scene.toml --output /tmp/kuwahara_r4.png --schemas schemas --width 1280 --height 720 --kuwahara-radius 4`

**Step 3: Compare outputs**

View both PNGs and verify:
- `no_kuwahara.png` looks normal
- `kuwahara_r4.png` has a visible painterly/oil-painting effect
- No rendering artifacts, black screens, or GPU errors

**Step 4: Test with different radii**

Run: `cargo run --bin flint -- render demo/phase4_runtime.scene.toml --output /tmp/kuwahara_r8.png --schemas schemas --width 1280 --height 720 --kuwahara-radius 8`

Verify larger radius produces a more pronounced effect.

**Step 5: Test anisotropy dial**

Run: `cargo run --bin flint -- render demo/phase4_runtime.scene.toml --output /tmp/kuwahara_iso.png --schemas schemas --width 1280 --height 720 --kuwahara-radius 4 --kuwahara-anisotropy 0`

Verify `anisotropy 0` produces a more uniform/circular filtering (less edge-following) compared to the default.

---

### Task 13: Update docs

Document the new effect in CLAUDE.md and the mdbook.

**Files:**
- Modify: `CLAUDE.md` — Add `--kuwahara-*` flags to the render command docs, F12 to the key list
- Check: `docs/` for any mdbook page documenting post-processing effects

**Step 1: Update CLAUDE.md render flags**

In the `flint render` docs section, add to the debug/post-processing flags list:

```
   #   --kuwahara-radius 4       --kuwahara-sharpness 8.0
   #   --kuwahara-hardness 8.0   --kuwahara-anisotropy 1.0
```

**Step 2: Update CLAUDE.md post-processing description**

In the post-processing bullet point, add "Kuwahara" to the list: "bloom, SSAO, fog, volumetric (god rays), Kuwahara, vignette"

**Step 3: Update CLAUDE.md F-key list**

Add in the input system or relevant section: F12 toggles Kuwahara filter.

**Step 4: Commit**

```
git add CLAUDE.md
git commit -m "Document Kuwahara filter in CLAUDE.md"
```
