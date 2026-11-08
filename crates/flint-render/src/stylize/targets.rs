//! Textures the stylize chain draws into: each stage's ping-pong pair and
//! the styles' own intermediates. They live in the post chain's resources,
//! which a resize rebuilds, so every one of them (and every bind group
//! cached over one) goes with the old size. A render-scale step keeps the
//! surface's size, so it keeps the set (#270): display-stage targets, and
//! the history in them, carry on, and HDR-stage ones are remade at the
//! scene's new size the next time they're asked for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A texture and its default view.
pub struct StyleTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Textures by name and format, made on first use.
#[derive(Default)]
pub struct StyleTargets {
    textures: Mutex<HashMap<(&'static str, wgpu::TextureFormat), Arc<StyleTexture>>>,
}

impl StyleTargets {
    /// The texture `name` in `format`, `width` x `height`. A name asked
    /// for at a second size is made again: an HDR-stage target after a
    /// render-scale step (the bind groups over the old one went with the
    /// resources it was kept from), or a caller drawing one chain at two
    /// sizes, which nothing does (so it's logged).
    pub fn get(
        &self,
        device: &wgpu::Device,
        name: &'static str,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Arc<StyleTexture> {
        let (width, height) = (width.max(1), height.max(1));
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = textures.get(&(name, format)) {
            if (t.width, t.height) == (width, height) {
                return t.clone();
            }
            tracing::debug!(
                "style target {name} asked for at {width}x{height}, made at {}x{}",
                t.width,
                t.height
            );
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let made = Arc::new(StyleTexture {
            texture,
            view,
            width,
            height,
        });
        textures.insert((name, format), made.clone());
        made
    }

    /// How many textures it holds.
    pub fn len(&self) -> usize {
        self.textures.lock().map_or(0, |t| t.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What it holds: how many textures, their bytes, and how many of them
    /// are `width` x `height` in `format` (the display stage's full-size
    /// textures, #302). Textures stay until a resize, so this is everything
    /// any configuration since then has asked for.
    pub fn summary(&self, format: wgpu::TextureFormat, width: u32, height: u32) -> TargetSummary {
        let Ok(textures) = self.textures.lock() else {
            return TargetSummary::default();
        };
        let mut summary = TargetSummary::default();
        for ((_, f), t) in textures.iter() {
            summary.count += 1;
            let texel = f.block_copy_size(None).unwrap_or(4);
            summary.bytes += u64::from(t.width) * u64::from(t.height) * u64::from(texel);
            if (*f, t.width, t.height) == (format, width, height) {
                summary.full_size += 1;
            }
        }
        summary
    }

    /// Every texture it holds, by name (#412): what each costs, for
    /// `--gpu-time` and the F3 panel.
    pub fn entries(&self) -> Vec<TargetEntry> {
        let Ok(textures) = self.textures.lock() else {
            return Vec::new();
        };
        let mut entries: Vec<TargetEntry> = textures
            .iter()
            .map(|((name, format), t)| TargetEntry {
                name,
                format: *format,
                width: t.width,
                height: t.height,
                bytes: u64::from(t.width)
                    * u64::from(t.height)
                    * u64::from(format.block_copy_size(None).unwrap_or(4)),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(b.name));
        entries
    }
}

/// One texture of [`StyleTargets::entries`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetEntry {
    pub name: &'static str,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

impl TargetEntry {
    /// `name  WxH  format  1.2 MB`, for a report.
    pub fn line(&self) -> String {
        format!(
            "{}  {}x{}  {:?}  {:.2} MB",
            self.name,
            self.width,
            self.height,
            self.format,
            self.bytes as f64 / (1024.0 * 1024.0)
        )
    }
}

/// [`StyleTargets::summary`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetSummary {
    pub count: u32,
    pub bytes: u64,
    pub full_size: u32,
}
