//! GPU texture cache — uploads imported textures and provides default fallbacks

use flint_core::vfs;
use flint_import::ImportedTexture;
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

pub use flint_pixels::{memoize_mip_chains, mip_chain};

/// Open and decode an image file through the file layer: `image::open`, but
/// the bytes come from [`vfs::read`] so a memory source works too. The format
/// comes from the path's extension, as `image::open` does, falling back to
/// sniffing the bytes when the extension names none.
pub fn open_image(path: &Path) -> image::ImageResult<image::DynamicImage> {
    let bytes = vfs::read(path).map_err(image::ImageError::IoError)?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    match image::ImageFormat::from_path(path) {
        Ok(format) => reader.set_format(format),
        Err(_) => reader = reader.with_guessed_format()?,
    }
    reader.decode()
}

/// A GPU-resident texture with its view and sampler
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    /// Texture width in pixels
    pub width: u32,
    /// Texture height in pixels
    pub height: u32,
}

/// Whether a loose image file is a data map, by its name: the companion
/// maps `<tex>_normal` and `<tex>_roughness` are sampled linear, anything
/// else as sRGB colour. Only for files on disk; a model's own textures go
/// by how its materials use them.
pub fn is_data_map_name(name: &str) -> bool {
    name.contains("normal") || name.contains("roughness")
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
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        GpuTexture {
            texture,
            view,
            sampler,
            width: 1,
            height: 1,
        }
    }

    /// Upload an imported texture to the GPU.
    ///
    /// `linear` is for data maps (normal, metallic-roughness): they're
    /// sampled as stored. Colour maps (base colour, emissive) are sRGB. The
    /// caller decides from how the material uses the texture, since a glTF
    /// texture's name says nothing about it. The whole mip chain is built
    /// (see [`mip_chain`]), so a model's painted texture seen from far off
    /// (generated ground, signage) settles instead of shimmering.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        imported: &ImportedTexture,
        linear: bool,
    ) {
        if self.textures.contains_key(name) {
            return;
        }

        // Convert to RGBA8 if needed
        let rgba_data = flint_pixels::to_rgba8(
            &imported.data,
            &imported.format,
            imported.width,
            imported.height,
        );

        let format = if linear {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };

        let (data, mip_level_count) =
            mip_chain(&rgba_data, imported.width, imported.height, !linear);

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d {
                    width: imported.width,
                    height: imported.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", name)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
                width: imported.width,
                height: imported.height,
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
        self.load_file_with(device, queue, name, path, false)
    }

    /// [`load_file`](Self::load_file) with a full mip chain, for surfaces
    /// seen at grazing angles and long distances (roads, floors) where a
    /// single level shimmers. Levels are box-filtered on the CPU, colour in
    /// linear light. If `name` is already cached it is kept as it is.
    pub fn load_file_mipmapped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        self.load_file_with(device, queue, name, path, true)
    }

    fn load_file_with(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
        mipmaps: bool,
    ) -> Result<bool, String> {
        if self.textures.contains_key(name) {
            return Ok(false);
        }

        let img = open_image(path)
            .map_err(|e| format!("Failed to open image '{}': {}", path.display(), e))?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();

        let is_data = is_data_map_name(name);
        let format = if is_data {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };

        let (data, mip_level_count) = if mipmaps {
            mip_chain(&rgba, width, height, !is_data)
        } else {
            (rgba.into_raw(), 1)
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
                mip_level_count,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", name)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
                width,
                height,
            },
        );

        Ok(true)
    }

    /// Upload raw RGBA8 pixel data to the GPU as a named texture.
    ///
    /// Returns `Ok(true)` if newly uploaded, `Ok(false)` if already cached.
    pub fn upload_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<bool, String> {
        if self.textures.contains_key(name) {
            return Ok(false);
        }

        let expected = (width * height * 4) as usize;
        if data.len() != expected {
            return Err(format!(
                "RGBA data length {} doesn't match {}x{}x4 = {}",
                data.len(),
                width,
                height,
                expected
            ));
        }

        let format = if is_normal_map {
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
            data,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} Sampler", name)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
                width,
                height,
            },
        );

        Ok(true)
    }

    /// Check if a texture is already cached by name.
    pub fn contains(&self, name: &str) -> bool {
        self.textures.contains_key(name)
    }

    /// Get a texture by name, returning None if not found
    pub fn get(&self, name: &str) -> Option<&GpuTexture> {
        self.textures.get(name)
    }

    /// Get the pixel dimensions of a cached texture
    pub fn get_dimensions(&self, name: &str) -> Option<(u32, u32)> {
        self.textures.get(name).map(|t| (t.width, t.height))
    }

    /// Update an existing texture's pixel data in-place on the GPU.
    ///
    /// The texture must already exist and the data must match its dimensions.
    /// Uses `queue.write_texture()` — no bind group rebuild needed.
    pub fn update_rgba(&self, queue: &wgpu::Queue, name: &str, data: &[u8]) -> Result<(), String> {
        let tex = self
            .textures
            .get(name)
            .ok_or_else(|| format!("Texture '{}' not found in cache", name))?;

        let expected = (tex.width * tex.height * 4) as usize;
        if data.len() != expected {
            return Err(format!(
                "Data length {} doesn't match {}x{}x4 = {}",
                data.len(),
                tex.width,
                tex.height,
                expected
            ));
        }

        // A mipmapped texture gets its whole chain rebuilt, or the smaller
        // levels would go on showing the old image at a distance.
        let levels = tex.texture.mip_level_count();
        let chain;
        let data = if levels > 1 {
            let srgb = tex.texture.format() == wgpu::TextureFormat::Rgba8UnormSrgb;
            chain = mip_chain(data, tex.width, tex.height, srgb).0;
            chain.as_slice()
        } else {
            data
        };

        let (mut w, mut h, mut offset) = (tex.width, tex.height, 0usize);
        for level in 0..levels {
            let len = (w * h * 4) as usize;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data[offset..offset + len],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            offset += len;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }

        Ok(())
    }

    /// Remove a texture from the cache by name.
    pub fn remove_texture(&mut self, name: &str) {
        self.textures.remove(name);
    }

    /// Clear all user-loaded textures (preserves built-in defaults)
    pub fn clear_user_textures(&mut self) {
        self.textures.clear();
    }
}
