//! GPU texture cache — uploads imported textures and provides default fallbacks

use flint_import::ImportedTexture;
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

/// A GPU-resident texture with its view and sampler
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

/// Cache of GPU textures, keyed by name, with built-in defaults
pub struct TextureCache {
    textures: HashMap<String, GpuTexture>,
    /// 1x1 white texture (default base color)
    pub default_white: GpuTexture,
    /// 1x1 flat normal map (0.5, 0.5, 1.0) = straight up
    pub default_normal: GpuTexture,
    /// 1x1 default metallic-roughness (metallic=0, roughness=0.5)
    pub default_metallic_roughness: GpuTexture,
}

impl TextureCache {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let default_white = Self::create_1x1(device, queue, [255, 255, 255, 255], "Default White");
        let default_normal =
            Self::create_1x1(device, queue, [128, 128, 255, 255], "Default Normal");
        // Green channel = roughness 0.5 (128/255), Blue channel = metallic 0.0 (0/255)
        // glTF packs metallic in blue, roughness in green
        let default_metallic_roughness =
            Self::create_1x1(device, queue, [0, 128, 0, 255], "Default MR");

        Self {
            textures: HashMap::new(),
            default_white,
            default_normal,
            default_metallic_roughness,
        }
    }

    fn create_1x1(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: [u8; 4],
        label: &str,
    ) -> GpuTexture {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &color,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", label)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        GpuTexture {
            texture,
            view,
            sampler,
        }
    }

    /// Upload an imported texture to the GPU
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        imported: &ImportedTexture,
    ) {
        if self.textures.contains_key(name) {
            return;
        }

        // Convert to RGBA8 if needed
        let rgba_data = Self::ensure_rgba(&imported.data, &imported.format, imported.width, imported.height);

        // Determine if this is a normal map (use Rgba8Unorm, not sRGB)
        let format = if name.contains("normal") {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d {
                    width: imported.width,
                    height: imported.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &rgba_data,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", name)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        self.textures.insert(
            name.to_string(),
            GpuTexture {
                texture,
                view,
                sampler,
            },
        );
    }

    /// Load a texture from an image file on disk.
    /// Returns Ok(true) if newly loaded, Ok(false) if already cached.
    pub fn load_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if self.textures.contains_key(name) {
            return Ok(false);
        }

        let img = image::open(path)
            .map_err(|e| format!("Failed to open image '{}': {}", path.display(), e))?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();

        let format = if name.contains("normal") {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &rgba,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", name)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        self.textures.insert(
            name.to_string(),
            GpuTexture {
                texture,
                view,
                sampler,
            },
        );

        Ok(true)
    }

    /// Get a texture by name, returning None if not found
    pub fn get(&self, name: &str) -> Option<&GpuTexture> {
        self.textures.get(name)
    }

    /// Convert imported texture data to RGBA8 format
    fn ensure_rgba(data: &[u8], format: &str, width: u32, height: u32) -> Vec<u8> {
        let pixel_count = (width * height) as usize;
        match format {
            "rgba8" => data.to_vec(),
            "rgb8" => {
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for chunk in data.chunks_exact(3) {
                    rgba.push(chunk[0]);
                    rgba.push(chunk[1]);
                    rgba.push(chunk[2]);
                    rgba.push(255);
                }
                rgba
            }
            "rg8" => {
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for chunk in data.chunks_exact(2) {
                    rgba.push(chunk[0]);
                    rgba.push(chunk[1]);
                    rgba.push(0);
                    rgba.push(255);
                }
                rgba
            }
            "r8" => {
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for &byte in data {
                    rgba.push(byte);
                    rgba.push(byte);
                    rgba.push(byte);
                    rgba.push(255);
                }
                rgba
            }
            _ => {
                // For unsupported formats, return white pixels
                vec![255u8; pixel_count * 4]
            }
        }
    }
}
