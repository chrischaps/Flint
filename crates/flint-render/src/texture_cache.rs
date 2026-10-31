//! GPU texture cache — uploads imported textures and provides default fallbacks

use flint_core::vfs;
use flint_import::ImportedTexture;
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

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
            mipmap_filter: wgpu::FilterMode::Nearest,
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
    /// texture's name says nothing about it.
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
        let rgba_data = Self::ensure_rgba(
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
                wgpu::ImageCopyTexture {
                    texture: &tex.texture,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data[offset..offset + len],
                wgpu::ImageDataLayout {
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

/// Number of mip levels down to 1x1 for a `width` x `height` image.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// A full mip chain for RGBA8 pixels: level 0 (the input) first, then each
/// half-size level, all concatenated in the order
/// `create_texture_with_data` expects. Each level is a 2x2 box filter of the
/// one above (an odd edge repeats its last texel). With `srgb` the colour
/// channels are averaged in linear light, so thin bright paint fades into
/// the ground at a distance instead of darkening it; alpha is always linear.
/// Returns the bytes and the level count.
pub fn mip_chain(rgba: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32) {
    let levels = mip_level_count(width, height);
    let to_linear: Vec<f32> = (0..256)
        .map(|i| {
            let c = i as f32 / 255.0;
            if !srgb {
                c
            } else if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    let to_byte = |v: f32, colour: bool| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let e = if colour && srgb {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        } else {
            v
        };
        (e * 255.0 + 0.5) as u8
    };

    let mut out = rgba.to_vec();
    let mut cur: Vec<f32> = rgba
        .chunks_exact(4)
        .flat_map(|p| {
            [
                to_linear[p[0] as usize],
                to_linear[p[1] as usize],
                to_linear[p[2] as usize],
                p[3] as f32 / 255.0,
            ]
        })
        .collect();
    let (mut w, mut h) = (width as usize, height as usize);
    for _ in 1..levels {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0.0f32; nw * nh * 4];
        for y in 0..nh {
            let (y0, y1) = ((2 * y).min(h - 1), (2 * y + 1).min(h - 1));
            for x in 0..nw {
                let (x0, x1) = ((2 * x).min(w - 1), (2 * x + 1).min(w - 1));
                for c in 0..4 {
                    next[(y * nw + x) * 4 + c] = 0.25
                        * (cur[(y0 * w + x0) * 4 + c]
                            + cur[(y0 * w + x1) * 4 + c]
                            + cur[(y1 * w + x0) * 4 + c]
                            + cur[(y1 * w + x1) * 4 + c]);
                }
            }
        }
        out.extend(next.chunks_exact(4).flat_map(|p| {
            [
                to_byte(p[0], true),
                to_byte(p[1], true),
                to_byte(p[2], true),
                to_byte(p[3], false),
            ]
        }));
        cur = next;
        w = nw;
        h = nh;
    }
    (out, levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_counts() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(1024, 1024), 11);
        assert_eq!(mip_level_count(1024, 256), 11);
        assert_eq!(mip_level_count(5, 3), 3);
    }

    #[test]
    fn chain_sizes_add_up() {
        let (w, h) = (6u32, 3u32);
        let (data, levels) = mip_chain(&vec![200u8; (w * h * 4) as usize], w, h, true);
        // 6x3, 3x1, 1x1
        assert_eq!(levels, 3);
        assert_eq!(data.len(), (18 + 3 + 1) * 4);
        // a flat colour stays the same colour all the way down
        assert!(data.iter().all(|&b| b == 200));
    }

    #[test]
    fn srgb_average_is_in_linear_light() {
        // black and white average to linear 0.5, which is sRGB 188
        let px = [0, 0, 0, 255, 255, 255, 255, 255];
        let (data, _) = mip_chain(&px, 2, 1, true);
        assert_eq!(&data[8..11], &[188, 188, 188]);
        assert_eq!(data[11], 255);
        let (data, _) = mip_chain(&px, 2, 1, false);
        assert_eq!(data[8], 128);
    }
}
