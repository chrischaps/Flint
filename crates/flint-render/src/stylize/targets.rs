//! Textures the stylize chain draws into: each stage's ping-pong pair and
//! the styles' own intermediates. They live in the post chain's resources,
//! which a resize rebuilds, so every one of them (and every bind group
//! cached over one) goes with the old size.

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
    /// for at a second size is made again; that only happens if a caller
    /// draws one chain at two sizes, which nothing does (bind groups over
    /// the old one would then be stale, so it's logged).
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
}
