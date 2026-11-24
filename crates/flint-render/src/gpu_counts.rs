//! The GPU objects Flint's renderer creates and the bytes it uploads,
//! counted natively (#1091).
//!
//! Per-frame GPU allocations are the renderer's CPU cliff
//! (`crates/flint-render/CLAUDE.md`), and until #1091 only the web page
//! could see them, by wrapping WebGPU's methods in JavaScript (#139). Every
//! `create_buffer`, `create_buffer_init`, `create_bind_group`,
//! `create_texture`, `create_texture_with_data`, `create_render_pipeline`,
//! `create_compute_pipeline`, `queue.write_buffer` and `queue.write_texture`
//! in this crate goes through [`CountedDevice`] / [`CountedQueue`] instead,
//! and shader modules are counted where they are made
//! ([`TrustedShader`](crate::TrustedShader) and the user-style path). The
//! crate's own `clippy.toml` (written by `scripts/clippy-conf.sh` from
//! `scripts/clippy-render.toml`) refuses the bare calls, so a new one can't
//! slip past the count and read as a false all-clear.
//!
//! The counts are process-wide running totals in relaxed atomics: no lock
//! and no allocation, one `fetch_add` a call. A frame's count is the
//! difference between two readings ([`GpuCalls::since`]);
//! [`SceneRenderer::frame_submitted`](crate::SceneRenderer::frame_submitted)
//! takes one each frame. Other crates' objects (egui's, the player's own
//! targets) are not counted: this is what `flint-render` makes.
//!
//! Live memory is the other half: [`GpuMemory`] is what the renderer knows
//! it holds, by class, walked from its own fields
//! ([`SceneRenderer::gpu_memory`](crate::SceneRenderer::gpu_memory)), and
//! [`device_memory`] the device's own totals where wgpu tracks them
//! (wgpu's `counters` feature; Vulkan and DX12 natively, nothing on the
//! web), which shows what the walk doesn't know.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Running totals, or the difference between two (a frame's calls).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GpuCalls {
    /// `create_buffer` and `create_buffer_init`, and their sizes.
    pub buffers: u64,
    pub buffer_bytes: u64,
    pub bind_groups: u64,
    /// `create_texture` and `create_texture_with_data`, and what each
    /// holds (every mip and layer, [`texture_bytes`]).
    pub textures: u64,
    pub texture_bytes: u64,
    /// Render and compute pipelines built (a `pipeline_cache` hit builds
    /// none).
    pub pipelines: u64,
    pub shader_modules: u64,
    /// `queue.write_buffer`, and the bytes it carried.
    pub writes: u64,
    pub write_bytes: u64,
    /// `queue.write_texture`, and the bytes it carried.
    pub texture_writes: u64,
    pub texture_write_bytes: u64,
}

const N: usize = 11;
static TOTALS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

const BUFFERS: usize = 0;
const BUFFER_BYTES: usize = 1;
const BIND_GROUPS: usize = 2;
const TEXTURES: usize = 3;
const TEXTURE_BYTES: usize = 4;
const PIPELINES: usize = 5;
const SHADER_MODULES: usize = 6;
const WRITES: usize = 7;
const WRITE_BYTES: usize = 8;
const TEXTURE_WRITES: usize = 9;
const TEXTURE_WRITE_BYTES: usize = 10;

#[inline]
fn add(at: usize, n: u64) {
    TOTALS[at].fetch_add(n, Relaxed);
}

/// What the process's renderers have made and written so far.
pub fn totals() -> GpuCalls {
    let t = |i: usize| TOTALS[i].load(Relaxed);
    GpuCalls {
        buffers: t(BUFFERS),
        buffer_bytes: t(BUFFER_BYTES),
        bind_groups: t(BIND_GROUPS),
        textures: t(TEXTURES),
        texture_bytes: t(TEXTURE_BYTES),
        pipelines: t(PIPELINES),
        shader_modules: t(SHADER_MODULES),
        writes: t(WRITES),
        write_bytes: t(WRITE_BYTES),
        texture_writes: t(TEXTURE_WRITES),
        texture_write_bytes: t(TEXTURE_WRITE_BYTES),
    }
}

/// A shader module made (the two places that make them call this).
pub(crate) fn count_shader_module() {
    add(SHADER_MODULES, 1);
}

impl GpuCalls {
    /// What was made and written between `then` and `self`.
    pub fn since(&self, then: &GpuCalls) -> GpuCalls {
        GpuCalls {
            buffers: self.buffers.saturating_sub(then.buffers),
            buffer_bytes: self.buffer_bytes.saturating_sub(then.buffer_bytes),
            bind_groups: self.bind_groups.saturating_sub(then.bind_groups),
            textures: self.textures.saturating_sub(then.textures),
            texture_bytes: self.texture_bytes.saturating_sub(then.texture_bytes),
            pipelines: self.pipelines.saturating_sub(then.pipelines),
            shader_modules: self.shader_modules.saturating_sub(then.shader_modules),
            writes: self.writes.saturating_sub(then.writes),
            write_bytes: self.write_bytes.saturating_sub(then.write_bytes),
            texture_writes: self.texture_writes.saturating_sub(then.texture_writes),
            texture_write_bytes: self
                .texture_write_bytes
                .saturating_sub(then.texture_write_bytes),
        }
    }

    /// GPU objects created: buffers, bind groups, textures, pipelines and
    /// shader modules. Zero on a steady frame; writes are not objects.
    pub fn created(&self) -> u64 {
        self.buffers + self.bind_groups + self.textures + self.pipelines + self.shader_modules
    }

    /// `3 buffers, 2 bind groups`: what was created, the kinds that were.
    /// Allocates; for a note or a report, never a steady frame's path.
    pub fn created_line(&self) -> String {
        let kinds = [
            (self.buffers, "buffer"),
            (self.bind_groups, "bind group"),
            (self.textures, "texture"),
            (self.pipelines, "pipeline"),
            (self.shader_modules, "shader module"),
        ];
        let parts: Vec<String> = kinds
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}{}", if *n == 1 { "" } else { "s" }))
            .collect();
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        }
    }
}

impl std::ops::AddAssign for GpuCalls {
    /// Sum frames' calls (a report's total over a run).
    fn add_assign(&mut self, o: GpuCalls) {
        self.buffers += o.buffers;
        self.buffer_bytes += o.buffer_bytes;
        self.bind_groups += o.bind_groups;
        self.textures += o.textures;
        self.texture_bytes += o.texture_bytes;
        self.pipelines += o.pipelines;
        self.shader_modules += o.shader_modules;
        self.writes += o.writes;
        self.write_bytes += o.write_bytes;
        self.texture_writes += o.texture_writes;
        self.texture_write_bytes += o.texture_write_bytes;
    }
}

/// What a texture of this shape holds, bytes: every mip level, layer and
/// sample, at the format's size ([`wgpu::TextureFormat::theoretical_memory_footprint`];
/// drivers pad, so the device's own total reads higher).
pub fn texture_bytes(
    format: wgpu::TextureFormat,
    size: wgpu::Extent3d,
    dimension: wgpu::TextureDimension,
    mip_level_count: u32,
    sample_count: u32,
) -> u64 {
    let mut bytes = 0u64;
    for level in 0..mip_level_count.max(1) {
        let level_size = size.mip_level_size(level, dimension);
        bytes += format.theoretical_memory_footprint(level_size);
    }
    bytes * u64::from(sample_count.max(1))
}

/// [`texture_bytes`] of a texture that exists.
pub fn bytes_of(texture: &wgpu::Texture) -> u64 {
    texture_bytes(
        texture.format(),
        texture.size(),
        texture.dimension(),
        texture.mip_level_count(),
        texture.sample_count(),
    )
}

fn desc_bytes(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    texture_bytes(
        desc.format,
        desc.size,
        desc.dimension,
        desc.mip_level_count,
        desc.sample_count,
    )
}

/// The device's calls that make GPU objects, counted (#1091). The bare
/// `wgpu::Device` methods are refused in this crate by its `clippy.toml`.
pub trait CountedDevice {
    fn create_buffer_counted(&self, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer;
    fn create_buffer_init_counted(
        &self,
        desc: &wgpu::util::BufferInitDescriptor<'_>,
    ) -> wgpu::Buffer;
    fn create_bind_group_counted(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup;
    fn create_texture_counted(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture;
    fn create_texture_with_data_counted(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture;
    fn create_render_pipeline_counted(
        &self,
        desc: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline;
    fn create_compute_pipeline_counted(
        &self,
        desc: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline;
}

impl CountedDevice for wgpu::Device {
    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_buffer_counted(&self, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
        add(BUFFERS, 1);
        add(BUFFER_BYTES, desc.size);
        self.create_buffer(desc)
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_buffer_init_counted(
        &self,
        desc: &wgpu::util::BufferInitDescriptor<'_>,
    ) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        add(BUFFERS, 1);
        add(BUFFER_BYTES, desc.contents.len() as u64);
        self.create_buffer_init(desc)
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_bind_group_counted(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup {
        add(BIND_GROUPS, 1);
        self.create_bind_group(desc)
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_texture_counted(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture {
        add(TEXTURES, 1);
        add(TEXTURE_BYTES, desc_bytes(desc));
        self.create_texture(desc)
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_texture_with_data_counted(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture {
        use wgpu::util::DeviceExt;
        add(TEXTURES, 1);
        add(TEXTURE_BYTES, desc_bytes(desc));
        add(TEXTURE_WRITES, 1);
        add(TEXTURE_WRITE_BYTES, data.len() as u64);
        self.create_texture_with_data(queue, desc, order, data)
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_render_pipeline_counted(
        &self,
        desc: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline {
        add(PIPELINES, 1);
        let pipeline = self.create_render_pipeline(desc);
        // A replay's overdraw view builds twins from it (#1203); off, one load
        crate::pipeline_twins::note(desc, &pipeline);
        pipeline
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn create_compute_pipeline_counted(
        &self,
        desc: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline {
        add(PIPELINES, 1);
        self.create_compute_pipeline(desc)
    }
}

/// The queue's uploads, counted (#1091).
pub trait CountedQueue {
    fn write_buffer_counted(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]);
    fn write_texture_counted(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    );
}

impl CountedQueue for wgpu::Queue {
    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn write_buffer_counted(
        &self,
        buffer: &wgpu::Buffer,
        offset: wgpu::BufferAddress,
        data: &[u8],
    ) {
        add(WRITES, 1);
        add(WRITE_BYTES, data.len() as u64);
        self.write_buffer(buffer, offset, data);
    }

    #[inline]
    #[allow(clippy::disallowed_methods)] // the counted call
    fn write_texture_counted(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        add(TEXTURE_WRITES, 1);
        add(TEXTURE_WRITE_BYTES, data.len() as u64);
        self.write_texture(texture, data, layout, size);
    }
}

/// Live texture memory the renderer knows it holds, by class, bytes
/// ([`SceneRenderer::gpu_memory`](crate::SceneRenderer::gpu_memory)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GpuMemory {
    /// The frame's own targets: HDR, depth copies, SSAO, volumetrics,
    /// bloom, MSAA, render scale, water, panels, the style albedo's MSAA.
    pub scene_targets: u64,
    /// The style chain's textures ([`SceneRenderer::style_target_entries`](crate::SceneRenderer::style_target_entries)).
    pub style_targets: u64,
    /// The shadow cascades' (and spots') depth array.
    pub shadow_maps: u64,
    /// Textures loaded for models and materials (the texture cache).
    pub model_textures: u64,
    /// Glyph atlases: ASCII's stacked fonts.
    pub glyph_atlases: u64,
    /// Mesh buffers the renderer holds: vertex and index buffers of loaded
    /// models (static and skinned), their bone buffers, and procedural
    /// boxes. The rest of its buffers (uniforms, the transform arena,
    /// particles) are in the device's total ([`device_memory`]).
    pub mesh_buffers: u64,
}

impl GpuMemory {
    /// Every texture class together.
    pub fn textures(&self) -> u64 {
        self.scene_targets
            + self.style_targets
            + self.shadow_maps
            + self.model_textures
            + self.glyph_atlases
    }
}

/// The device's live memory, bytes, as wgpu-hal tracks it: `(buffers,
/// textures)`. `None` where nothing is tracked (the web, GL, Metal), which
/// reads as two zeros. Everything on the device, egui's and the player's
/// own targets too, at the allocator's sizes (padded and aligned).
pub fn device_memory(device: &wgpu::Device) -> Option<(u64, u64)> {
    let hal = device.get_internal_counters().hal;
    let buffers = hal.buffer_memory.read().max(0) as u64;
    let textures = hal.texture_memory.read().max(0) as u64;
    (buffers > 0 || textures > 0).then_some((buffers, textures))
}

/// `12.3 MB` (MiB, one decimal), `640 KB` below one.
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frames_calls_are_the_difference_of_two_readings() {
        let then = GpuCalls {
            buffers: 10,
            bind_groups: 4,
            writes: 100,
            write_bytes: 4096,
            ..Default::default()
        };
        let now = GpuCalls {
            buffers: 12,
            bind_groups: 4,
            writes: 115,
            write_bytes: 5120,
            pipelines: 1,
            ..Default::default()
        };
        let frame = now.since(&then);
        assert_eq!(frame.buffers, 2);
        assert_eq!(frame.bind_groups, 0);
        assert_eq!(frame.writes, 15);
        assert_eq!(frame.write_bytes, 1024);
        assert_eq!(
            frame.created(),
            3,
            "two buffers and a pipeline; writes aren't objects"
        );
        assert_eq!(frame.created_line(), "2 buffers, 1 pipeline");
        assert_eq!(GpuCalls::default().created_line(), "none");
        // A reading taken before another's never goes negative
        assert_eq!(then.since(&now).buffers, 0);
    }

    #[test]
    fn a_textures_bytes_count_every_mip_layer_and_sample() {
        let size = |w, h, d| wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: d,
        };
        let d2 = wgpu::TextureDimension::D2;
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        assert_eq!(texture_bytes(rgba, size(4, 4, 1), d2, 1, 1), 64);
        // 4x4 + 2x2 + 1x1 texels
        assert_eq!(
            texture_bytes(rgba, size(4, 4, 1), d2, 3, 1),
            (16 + 4 + 1) * 4
        );
        // Layers of an array stay at every level
        assert_eq!(texture_bytes(rgba, size(4, 4, 3), d2, 1, 1), 3 * 64);
        // Four samples
        assert_eq!(texture_bytes(rgba, size(4, 4, 1), d2, 1, 4), 4 * 64);
        // A depth format has no copy size; the footprint's estimate is used
        let depth = wgpu::TextureFormat::Depth32Float;
        assert_eq!(texture_bytes(depth, size(8, 8, 1), d2, 1, 1), 256);
        assert_eq!(
            texture_bytes(wgpu::TextureFormat::Rgba16Float, size(2, 2, 1), d2, 1, 1),
            32
        );
    }

    #[test]
    fn bytes_read_as_mb_or_kb() {
        assert_eq!(format_bytes(512 * 1024), "512 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 300 * 1024), "3.3 MB");
    }
}
