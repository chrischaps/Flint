//! The glyph atlas on the GPU: one `R8Unorm` texture, uploaded once.

use super::GlyphAtlas;

/// A [`GlyphAtlas`] as a coverage texture: sample `.r` for ink (0..1). Cells
/// sit at [`GlyphAtlas::cell_origin`] with a one-pixel gutter, so nearest or
/// linear sampling inside a cell never reads a neighbour.
pub struct GpuGlyphAtlas {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// Atlas size in pixels.
    pub size: [u32; 2],
    /// Cell size in pixels.
    pub cell: [u32; 2],
    /// Cells per row.
    pub columns: u32,
}

impl GpuGlyphAtlas {
    /// Upload `atlas`. Build one per atlas and keep it (the atlas never
    /// changes), never per frame.
    pub fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &GlyphAtlas,
        label: &str,
    ) -> Self {
        let [width, height] = atlas.size();
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            atlas.pixels(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size: [width, height],
            cell: atlas.cell_size(),
            columns: atlas.columns(),
        }
    }
}
