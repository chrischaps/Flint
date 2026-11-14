//! A second view of the same frame, for a split screen (#238): the scene drawn
//! again into a texture of the window's size, and the part of it right of a seam
//! copied over the frame already drawn.
//!
//! The copy reads and writes the texture's bytes as they are: both views are the
//! window format without its sRGB suffix, so nothing is decoded or encoded on the
//! way, and a pixel arrives exactly as the second drawing made it. That's what
//! lets each half of a split match a whole frame drawn the same way.

/// The second drawing and what copies it in.
pub struct SplitView {
    width: u32,
    height: u32,
    scene_format: wgpu::TextureFormat,
    copy_format: wgpu::TextureFormat,
    _texture: wgpu::Texture,
    scene_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl SplitView {
    /// Room for a frame of `width` x `height`, drawn in `scene_format` (the format
    /// the scene's last pass writes, sRGB on a desktop) and copied into a view in
    /// `copy_format`: the same format without its sRGB suffix, as the HUD's is.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        scene_format: wgpu::TextureFormat,
        copy_format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Split View Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = device.create_module(wgpu::ShaderModuleDescriptor {
            label: Some("Split View Copy"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Split View Pipeline Layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Split View Copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: copy_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let (texture, scene_view, bind_group) = Self::target(
            device,
            &layout,
            width.max(1),
            height.max(1),
            scene_format,
            copy_format,
        );
        Self {
            width: width.max(1),
            height: height.max(1),
            scene_format,
            copy_format,
            _texture: texture,
            scene_view,
            bind_group,
            layout,
            pipeline,
        }
    }

    fn target(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        width: u32,
        height: u32,
        scene_format: wgpu::TextureFormat,
        copy_format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
        let view_formats: &[wgpu::TextureFormat] = if copy_format != scene_format {
            &[copy_format]
        } else {
            &[]
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Split View Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: scene_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats,
        });
        let scene_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let copy_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(copy_format),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Split View Bind Group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&copy_view),
            }],
        });
        (texture, scene_view, bind_group)
    }

    /// Make sure it's `width` x `height`, building it again if the window has
    /// changed size. Nothing is made while it stays the same.
    pub fn ensure_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        let (texture, scene_view, bind_group) = Self::target(
            device,
            &self.layout,
            width,
            height,
            self.scene_format,
            self.copy_format,
        );
        self._texture = texture;
        self.scene_view = scene_view;
        self.bind_group = bind_group;
        self.width = width;
        self.height = height;
    }

    /// Where the second drawing goes: pass it to `SceneRenderer::encode_to` as
    /// the target, with the frame's own depth view.
    pub fn scene_view(&self) -> &wgpu::TextureView {
        &self.scene_view
    }

    /// The pixel column the second drawing takes over from, for a seam at `split`
    /// of the way across (0.5 is the middle).
    pub fn seam_x(&self, split: f32) -> u32 {
        ((self.width as f32 * split.clamp(0.0, 1.0)).round() as u32).min(self.width)
    }

    /// Copy columns `from_x` up to `to_x` of the second drawing over `target`, the
    /// frame in [`Self::new`]'s `copy_format`.
    pub fn encode_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        from_x: u32,
        to_x: u32,
    ) {
        let to_x = to_x.min(self.width);
        if from_x >= to_x {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Split View Copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_scissor_rect(from_x, 0, to_x - from_x, self.height);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

const SHADER: &str = r#"
@group(0) @binding(0) var drawn: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(drawn, vec2<i32>(position.xy), 0);
}
"#;

use crate::trusted_shader::TrustedShader as _;
