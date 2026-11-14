//! The style albedo (#869): each opaque mesh's own colour, before any light
//! reaches it, drawn once more after the scene pass for the styles that ask
//! for it ([`StyleDescriptor::uses_albedo`]).
//!
//! A style at the display stage sees the finished picture, which is albedo
//! times light. A pencil that hatches by the picture's luminance takes a
//! dark material for a shadow and a light prop in deep shadow for lit. The
//! picture divided by this target is the light alone, so a style can shade
//! where the light falls off, whatever the paint.
//!
//! The pass redraws the frame's opaque static, procedural and skinned draws
//! with their cached transform and material bind groups (as the veil, #290,
//! draws transparents), with the scene's own vertex stages and a fragment
//! stage (`fs_albedo` in `shader.wgsl` / `skinned_shader.wgsl`) that only
//! builds the base colour: no lights, no shadow taps. It tests against the
//! depth the scene drew (read-only, `LessEqual`, so the same vertices land
//! on their own depth), 4x against the scene's samples under MSAA and
//! resolved, so its silhouettes are the picture's. Terrain, grass, the
//! ocean, billboards and particles aren't drawn: their pixels keep alpha 0,
//! and a reader falls back on the picture there.
//!
//! [`StyleDescriptor::uses_albedo`]: crate::stylize::StyleDescriptor::uses_albedo

use crate::TrustedShader;
/// The albedo's name in the post chain's style targets.
pub(crate) const ALBEDO_TARGET: &str = "style albedo";

/// The albedo's format: linear base colour, alpha 1 where a mesh was drawn.
pub const ALBEDO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The scene depth's format, which the pass tests against.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// One opaque mesh, with its cached transform and material bind groups.
pub(crate) struct AlbedoMesh<'a> {
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
    pub index_count: u32,
    pub transform_bind_group: &'a wgpu::BindGroup,
    pub material_bind_group: &'a wgpu::BindGroup,
    /// The bone matrices, for a skinned draw.
    pub bones: Option<&'a wgpu::BindGroup>,
}

/// The bind group layouts the pass shares with the scene's pipelines.
pub(crate) struct AlbedoLayouts<'a> {
    pub transform: &'a wgpu::BindGroupLayout,
    pub material: &'a wgpu::BindGroupLayout,
    pub light: &'a wgpu::BindGroupLayout,
    pub bones: &'a wgpu::BindGroupLayout,
}

/// The static and skinned pipelines at one sample count.
struct Pipelines {
    samples: u32,
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
}

/// The pass: its pipelines per sample count, made on first use, and the
/// 4x target under MSAA.
pub struct StyleAlbedoPass {
    pbr: wgpu::ShaderModule,
    skinned: wgpu::ShaderModule,
    pipelines: Vec<Pipelines>,
    /// The multisampled target the 4x path draws into and resolves from,
    /// with its size.
    msaa_target: Option<(wgpu::Texture, wgpu::TextureView, u32, u32, u32)>,
    /// Opaque draws the last frame drew.
    last_draws: usize,
}

fn pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    samples: u32,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            buffers: &buffers.iter().cloned().map(Some).collect::<Vec<_>>(),
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_albedo"),
            targets: &[Some(wgpu::ColorTargetState {
                format: ALBEDO_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        // Both faces: a back face is behind the front one of a closed mesh
        // and fails the depth test, and a double-sided card's back is the
        // surface the scene drew
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

impl StyleAlbedoPass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let pbr = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Albedo PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let skinned = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Albedo Skinned Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("skinned_shader.wgsl").into()),
        });
        Self {
            pbr,
            skinned,
            pipelines: Vec::new(),
            msaa_target: None,
            last_draws: 0,
        }
    }

    /// How many opaque draws the last frame drew into the albedo.
    pub fn last_draws(&self) -> usize {
        self.last_draws
    }

    fn pipelines(
        &mut self,
        device: &wgpu::Device,
        layouts: &AlbedoLayouts<'_>,
        samples: u32,
    ) -> usize {
        if let Some(i) = self.pipelines.iter().position(|p| p.samples == samples) {
            return i;
        }
        let static_pipeline = pipeline(
            device,
            "Style Albedo Pipeline",
            &self.pbr,
            "vs_main",
            &[layouts.transform, layouts.material, layouts.light],
            &[crate::primitives::Vertex::desc()],
            samples,
        );
        let skinned_pipeline = pipeline(
            device,
            "Style Albedo Skinned Pipeline",
            &self.skinned,
            "vs_skinned",
            &[
                layouts.transform,
                layouts.material,
                layouts.light,
                layouts.bones,
            ],
            &[crate::primitives::SkinnedVertex::desc()],
            samples,
        );
        self.pipelines.push(Pipelines {
            samples,
            static_pipeline,
            skinned_pipeline,
        });
        self.pipelines.len() - 1
    }

    /// Clear `target` and draw `meshes` into it, tested against
    /// `scene_depth`: the single-sample depth, or (with `samples` > 1) the
    /// scene's multisampled one, the pass then resolving into `target`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layouts: &AlbedoLayouts<'_>,
        target: &crate::stylize::StyleTexture,
        scene_depth: &wgpu::TextureView,
        samples: u32,
        light_bind_group: &wgpu::BindGroup,
        meshes: &[AlbedoMesh<'_>],
    ) {
        self.last_draws = meshes.len();
        let samples = samples.max(1);
        let which = self.pipelines(device, layouts, samples);
        if samples > 1 {
            let size = (target.width, target.height, samples);
            let fits = self
                .msaa_target
                .as_ref()
                .is_some_and(|(_, _, w, h, s)| (*w, *h, *s) == size);
            if !fits {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Style Albedo MSAA"),
                    size: wgpu::Extent3d {
                        width: target.width,
                        height: target.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: ALBEDO_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                self.msaa_target = Some((texture, view, size.0, size.1, size.2));
            }
        }
        let (view, resolve_target) = match (&self.msaa_target, samples > 1) {
            (Some((_, ms_view, ..)), true) => (ms_view, Some(&target.view)),
            _ => (&target.view, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Albedo Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: if resolve_target.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            // Read-only: the scene's depth, which nothing here writes
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: scene_depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let pipelines = &self.pipelines[which];
        let mut skinned = None;
        for m in meshes {
            let is_skinned = m.bones.is_some();
            if skinned != Some(is_skinned) {
                pass.set_pipeline(if is_skinned {
                    &pipelines.skinned_pipeline
                } else {
                    &pipelines.static_pipeline
                });
                pass.set_bind_group(2, light_bind_group, &[]);
                skinned = Some(is_skinned);
            }
            pass.set_bind_group(0, m.transform_bind_group, &[]);
            pass.set_bind_group(1, m.material_bind_group, &[]);
            if let Some(bones) = m.bones {
                pass.set_bind_group(3, bones, &[]);
            }
            pass.set_vertex_buffer(0, m.vertex_buffer.slice(..));
            pass.set_index_buffer(m.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
    }
}
