//! Render scale (#122): the scene's HDR chain drawn at a fraction of the
//! surface's pixels and upscaled by the composite pass, which samples the
//! HDR buffer by UV into the full-size surface. Everything drawn after it
//! (world lines, world text, egui) stays at the surface's full resolution.
//!
//! The scene passes then need a depth buffer of the HDR chain's size, which
//! the renderer keeps itself ([`ScaledDepth`]); once the frame is drawn,
//! [`DepthUpscale`] copies it into the caller's surface-sized depth view,
//! nearest texel, so the overlays drawn against it still are.

/// The smallest scale [`SceneRenderer::set_render_scale`] accepts.
///
/// [`SceneRenderer::set_render_scale`]: crate::SceneRenderer::set_render_scale
pub const MIN_RENDER_SCALE: f32 = 0.25;

/// A scale clamped to what the renderer draws at: `MIN_RENDER_SCALE..=1`,
/// and 1 for anything that isn't a number.
pub fn clamp_render_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_RENDER_SCALE, 1.0)
    } else {
        1.0
    }
}

/// The scene's size for a surface of `width` x `height` at `scale`: each
/// side scaled and rounded, never below one pixel. Scale 1 is the surface.
pub fn scaled_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let scale = clamp_render_scale(scale);
    if scale >= 1.0 {
        return (width.max(1), height.max(1));
    }
    let side = |n: u32| ((n as f32 * scale).round() as u32).max(1);
    (side(width), side(height))
}

/// The scene's own depth buffer while it is drawn below the surface's size.
pub(crate) struct ScaledDepth {
    // Held so the view's texture lives as long as it
    _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl ScaledDepth {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Scaled Scene Depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
        }
    }
}

/// Copies the scaled scene depth into the surface-sized depth view: a
/// fullscreen triangle writing `frag_depth` from the nearest scene texel.
pub(crate) struct DepthUpscale {
    pipeline: wgpu::RenderPipeline,
    /// Bound to one [`ScaledDepth`]; rebuilt with it on a resize.
    bind_group: wgpu::BindGroup,
}

impl DepthUpscale {
    pub fn new(device: &wgpu::Device, source: &ScaledDepth) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Depth Upscale Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("depth_upscale.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Depth Upscale Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Depth Upscale Pipeline Layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Depth Upscale Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_upscale"),
                targets: &[],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Depth Upscale Bind Group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&source.view),
            }],
        });
        Self {
            pipeline,
            bind_group,
        }
    }

    /// Record the copy into `target`, which it overwrites.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Depth Upscale Pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_one_is_the_surface() {
        assert_eq!(scaled_size(2880, 1800, 1.0), (2880, 1800));
        assert_eq!(scaled_size(0, 0, 1.0), (1, 1));
    }

    #[test]
    fn a_dpr_two_screen_capped_at_one_and_a_half() {
        // A MacBook's 1440x900 CSS window at DPR 2, drawn as DPR 1.5
        let (w, h) = scaled_size(2880, 1800, 1.5 / 2.0);
        assert_eq!((w, h), (2160, 1350));
        let fraction = (w * h) as f64 / (2880.0 * 1800.0);
        assert!((fraction - 0.5625).abs() < 1e-6);
    }

    #[test]
    fn scales_are_clamped() {
        assert_eq!(clamp_render_scale(2.0), 1.0);
        assert_eq!(clamp_render_scale(0.0), MIN_RENDER_SCALE);
        assert_eq!(clamp_render_scale(f32::NAN), 1.0);
        assert_eq!(scaled_size(3, 3, 0.01), (1, 1));
    }
}
