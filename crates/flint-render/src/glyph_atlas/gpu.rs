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

/// Several glyph atlases stacked into one `R8Unorm` texture (#335), so one
/// pass can draw from more than one [`GlyphSet`](super::GlyphSet): atlas
/// `i` sits at `(0, tops[i])`. Each atlas keeps its own gutter on every
/// side, so a stack needs no gap between them and a glyph of one never
/// reads a neighbour from the next.
pub struct GpuGlyphStack {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// Stack size in pixels.
    pub size: [u32; 2],
    /// Each atlas's first row.
    pub tops: Vec<u32>,
}

/// The coverage bytes of `atlases` stacked top to bottom, the stack's size
/// (the widest atlas by every atlas's height) and each atlas's first row.
pub fn stack_pixels(atlases: &[&GlyphAtlas]) -> (Vec<u8>, [u32; 2], Vec<u32>) {
    let width = atlases
        .iter()
        .map(|a| a.size()[0])
        .max()
        .unwrap_or(1)
        .max(1);
    let mut tops = Vec::with_capacity(atlases.len());
    let mut height = 0;
    for a in atlases {
        tops.push(height);
        height += a.size()[1];
    }
    let height = height.max(1);
    let mut pixels = vec![0u8; (width * height) as usize];
    for (a, top) in atlases.iter().zip(&tops) {
        let [w, h] = a.size();
        for y in 0..h {
            let from = (y * w) as usize;
            let to = ((top + y) * width) as usize;
            pixels[to..to + w as usize].copy_from_slice(&a.pixels()[from..from + w as usize]);
        }
    }
    (pixels, [width, height], tops)
}

impl GpuGlyphStack {
    /// Upload `atlases` stacked. Build one when the sets in use change and
    /// keep it, never per frame.
    pub fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlases: &[&GlyphAtlas],
        label: &str,
    ) -> Self {
        let (pixels, [width, height], tops) = stack_pixels(atlases);
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
            &pixels,
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
            tops,
        }
    }
}
