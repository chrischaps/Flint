//! The MSAA targets (ADR 0058) and the depth resolve, owned by
//! [`MsaaTargets`] (ADR 0118, #736).
//!
//! With 4x on, the scene passes draw into a multisampled colour and depth
//! pair and resolve colour into the HDR buffer. Depth is resolved by hand:
//! a fullscreen pass copies sample 0 into the caller's single-sample depth
//! view after each opaque phase, so every depth consumer (SSAO, DoF, fog,
//! volumetric, the ocean's grab) stays single-sample and unchanged.

use crate::postprocess::HDR_FORMAT;
use crate::TrustedShader;

/// The renderer's MSAA state: the targets at the frame's size, and the
/// depth resolve's pipeline, made the first frame that draws 4x.
#[derive(Default)]
pub(crate) struct MsaaTargets {
    /// The targets at one view's size; each comic panel holds its own
    /// (`panels::ViewSlot`) and swaps it in around its draw.
    view: MsaaView,
    resolve: Option<DepthResolve>,
}

/// The multisampled colour and depth at one size.
#[derive(Default)]
pub(crate) struct MsaaView {
    color: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// A new id each time `depth` is made, so the style mask can keep a
    /// bind group over it (#306).
    depth_id: u64,
}

/// The sample-0 depth resolve's pipeline and its bind group layout.
struct DepthResolve {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl MsaaTargets {
    /// Make what a 4x frame of `width` x `height` draws into, if it isn't
    /// made at that size yet: the targets at `samples`, and the resolve.
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, samples: u32, width: u32, height: u32) {
        self.ensure_targets(device, samples, width, height);
        self.ensure_resolve_pipeline(device);
    }

    /// Drop the targets, so the next frame makes them at a new sample
    /// count (`set_msaa`). The resolve pipeline is single-sample and kept.
    pub(crate) fn drop_targets(&mut self) {
        self.view.color = None;
        self.view.depth = None;
    }

    /// Trade this view's targets for a panel's (`panels::ViewSlot`).
    pub(crate) fn swap_view(&mut self, other: &mut MsaaView) {
        std::mem::swap(&mut self.view, other);
    }

    /// The multisampled colour target. Made by [`Self::prepare`].
    pub(crate) fn color_view(&self) -> Option<&wgpu::TextureView> {
        self.view.color.as_ref().map(|(_, view, _, _)| view)
    }

    /// The multisampled depth target. Made by [`Self::prepare`].
    pub(crate) fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.view.depth.as_ref().map(|(_, view)| view)
    }

    /// The multisampled depth and its id, when it was made at `width` x
    /// `height`: what a pass drawing against the scene's own samples reads.
    pub(crate) fn depth_at(&self, width: u32, height: u32) -> Option<(&wgpu::TextureView, u64)> {
        match &self.view.depth {
            Some((texture, view))
                if texture.size().width == width && texture.size().height == height =>
            {
                Some((view, self.view.depth_id))
            }
            _ => None,
        }
    }

    /// Lazily (re)create the MSAA color+depth targets at the given size.
    fn ensure_targets(&mut self, device: &wgpu::Device, samples: u32, width: u32, height: u32) {
        if let Some((_, _, w, h)) = &self.view.color {
            if *w == width && *h == height {
                return;
            }
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Scene Color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Scene Depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        self.view.color = Some((color, color_view, width, height));
        self.view.depth = Some((depth, depth_view));
        self.view.depth_id = crate::bind_cache::gpu_id();
    }

    /// Lazily create the sample-0 depth-resolve pipeline (ADR 0058).
    fn ensure_resolve_pipeline(&mut self, device: &wgpu::Device) {
        if self.resolve.is_some() {
            return;
        }
        let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Depth Resolve Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../depth_resolve.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Depth Resolve Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Depth Resolve Pipeline Layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Depth Resolve Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_resolve"),
                targets: &[],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.resolve = Some(DepthResolve { pipeline, layout });
    }

    /// Copy sample 0 of the MSAA depth into a single-sample depth view via
    /// a fullscreen frag-depth pass, so SSAO/DoF/fog/volumetric/ocean-grab
    /// consumers stay unchanged (ADR 0058).
    pub(crate) fn resolve_depth_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target_depth_view: &wgpu::TextureView,
    ) {
        let Some(DepthResolve { pipeline, layout }) = &self.resolve else {
            return;
        };
        let msaa_depth_view = match &self.view.depth {
            Some((_, v)) => v,
            None => return,
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Depth Resolve Bind Group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(msaa_depth_view),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Depth Resolve Pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target_depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
