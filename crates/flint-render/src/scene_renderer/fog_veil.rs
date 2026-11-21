//! The fog veil (#513): what the composite needs to fog a translucent draw
//! at its own depth rather than at the depth of whatever lies behind it.
//!
//! Depth fog is drawn in the composite from the depth buffer, and the
//! translucent draws (blended meshes, living props' echoes, particles)
//! write no depth. Fogging a whole pixel by what's behind it made an echo
//! standing in front of a horizon two colours, with a hard line between:
//! fogged as far floor below the floor's edge, as unfogged sky above it.
//!
//! So while the fog reaches a translucent draw, the frame keeps two things
//! for the composite ([`FogVeilTargets`](crate::postprocess::FogVeilTargets)):
//!
//! - **behind**: the picture as it stood before the first translucent draw.
//!   The main pass ends there (after the opaque scene and the ocean), the
//!   HDR buffer is copied, and the pass goes on with the rest.
//! - **the veil**: each translucent draw once more, against the scene's
//!   depth, with no colour of its own but its weight and its depth:
//!   `r` = weight x (1 - depth), `g` = weight, `a` = the share of what's
//!   behind that still shows (cleared to 1, x (1 - alpha) by each draw that
//!   covers). It is "over" blended, so a near layer hides a far one's
//!   weight as it hides its colour. A draw that only adds light (additive,
//!   an echo's `lighten`) weighs in and covers nothing.
//!
//! The composite then splits a veiled pixel into two layers: what shows of
//! the picture behind (`a` x behind), fogged at the scene's depth as every
//! pixel was, and the rest, the translucent draws' own light, fogged at the
//! veil's depth `1 - r / g`. Each takes the fog's colour by its own share of
//! the pixel. A pixel no translucent draw reaches is fogged as before.
//!
//! Approximations: several translucent layers fog as one, at their
//! weighted depth (the nearest weighs most); woozy draws are drawn through
//! the plain vertex stage (the fog changes slowly with depth); a body
//! translucent only at its soft edge writes depth and is fogged as an
//! opaque one already. The veil is single-sample, like the style veil.

use super::{Blend, Draw, SceneRenderer};
use crate::gpu_counts::CountedDevice;
use crate::TrustedShader;

/// The veil's format: blendable everywhere, with half floats' precision on
/// `1 - depth`, which is small (about near / distance) where depth is fine.
pub(crate) const FOG_VEIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The scene depth the veil tests against.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Premultiplied "over" on the weights; the alpha keeps what still shows
/// of what's behind: `a' = a x (1 - src.a)`.
const FOG_VEIL_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// One kind of draw's two veil pipelines: one that covers, one that only
/// adds light.
struct Pair {
    covers: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
}

impl Pair {
    fn get(&self, light: bool) -> &wgpu::RenderPipeline {
        if light {
            &self.light
        } else {
            &self.covers
        }
    }
}

/// The veil's pipelines, made the first frame that needs them.
struct FogVeilPipelines {
    /// Blended static meshes: both faces, as the main pass draws an alpha
    /// mesh's back faces and then its front ones.
    meshes: Pair,
    skinned: Pair,
    /// Echoes: front faces only, as the main pass draws them.
    echoes: Pair,
    /// Made the first frame with particles (the particle pipeline is too).
    particles: Option<Pair>,
}

/// The renderer's share of the veil (ADR 0118: a subsystem's state in its
/// own struct): its pipelines, and whether this frame draws it.
#[derive(Default)]
pub(super) struct FogVeilState {
    pipelines: Option<FogVeilPipelines>,
    /// Set by `encode_to` for the frame it records: the main pass split at
    /// the first translucent draw and the veil was drawn, so the composite
    /// may read both.
    pub drawn: bool,
}

/// A veil pipeline: `module`'s `vs`/`fs`, single-sample, depth-tested
/// against the scene and never writing it.
fn pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    (vs, fs): (&str, &str),
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
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
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format: FOG_VEIL_FORMAT,
                blend: Some(FOG_VEIL_BLEND),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Both of a kind's pipelines.
fn pair(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    cull_mode: Option<wgpu::Face>,
) -> Pair {
    let make = |fs, which| {
        pipeline(
            device,
            &format!("Fog Veil {label} ({which})"),
            module,
            (vs, fs),
            layouts,
            buffers,
            cull_mode,
        )
    };
    Pair {
        covers: make("fs_fog_veil", "covers"),
        light: make("fs_fog_veil_light", "light"),
    }
}

/// Whether an item goes into the veil, and if so whether it only adds
/// light. Every match is spelled out, so a new kind of draw is asked.
fn veils(item: &super::DrawItem) -> Option<bool> {
    if !item.blend.blends() {
        return None;
    }
    let light = matches!(item.blend, Blend::Additive | Blend::Lighten);
    match &item.draw {
        Draw::Static(d) if d.is_wireframe => None,
        // A soft edge writes depth: the composite fogs it as opaque
        Draw::Skinned(d) if d.soft_edge_only => None,
        Draw::Static(_) | Draw::Skinned(_) | Draw::Echo(_) | Draw::Particles(_) => Some(light),
        // Billboards and terrain don't blend
        Draw::Billboard(_) | Draw::Terrain(_) => None,
    }
}

impl SceneRenderer {
    /// Whether this frame's fog reaches a translucent draw (#513): fog or
    /// ground fog on, and something drawn that blends. Read after the
    /// camera's culls.
    pub(super) fn fog_meets_translucents(&self) -> bool {
        let c = &self.postprocess_config;
        let ground = self.ground_fog.is_some() && c.ground_fog_density > 0.0;
        c.enabled
            && (c.fog_enabled || ground)
            && self.draws.drawn().any(|item| veils(item).is_some())
    }

    /// Draw the veil (#513) into the chain's target, against the frame's
    /// single-sample `depth_view`, once the scene passes are done. Whether
    /// it was drawn: the composite reads it only then.
    pub(super) fn encode_fog_veil(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
    ) -> bool {
        flint_core::scope!("encode_fog_veil");
        let Some(skinned) = &self.pipelines.skinned else {
            return false;
        };
        if self.fog_veil.pipelines.is_none() {
            let pbr = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Fog Veil PBR Shader"),
                source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SCENE.into()),
            });
            let skinned_module =
                device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Fog Veil Skinned Shader"),
                    source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SKINNED.into()),
                });
            let l = &self.layouts;
            let mesh_layouts = [
                &l.transform_bind_group_layout,
                &l.material_bind_group_layout,
                &l.light_bind_group_layout,
            ];
            let skinned_layouts = [
                &l.transform_bind_group_layout,
                &l.material_bind_group_layout,
                &l.light_bind_group_layout,
                &skinned.bone_bind_group_layout,
            ];
            let skinned_vertex = [crate::primitives::SkinnedVertex::desc()];
            self.fog_veil.pipelines = Some(FogVeilPipelines {
                meshes: pair(
                    device,
                    "Mesh",
                    &pbr,
                    "vs_main",
                    &mesh_layouts,
                    &[crate::primitives::Vertex::desc()],
                    None,
                ),
                skinned: pair(
                    device,
                    "Skinned",
                    &skinned_module,
                    "vs_skinned",
                    &skinned_layouts,
                    &skinned_vertex,
                    None,
                ),
                echoes: pair(
                    device,
                    "Echo",
                    &skinned_module,
                    "vs_skinned",
                    &skinned_layouts,
                    &skinned_vertex,
                    Some(wgpu::Face::Back),
                ),
                particles: None,
            });
        }
        let has_particles = self
            .draws
            .drawn()
            .any(|i| matches!(i.draw, Draw::Particles(_)));
        if let (Some(pipelines), Some(pp), true) = (
            self.fog_veil.pipelines.as_mut(),
            self.pipelines.particle.as_deref(),
            has_particles,
        ) {
            if pipelines.particles.is_none() {
                let module = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Fog Veil Particle Shader"),
                    source: wgpu::ShaderSource::Wgsl(
                        include_str!("../particle_shader.wgsl").into(),
                    ),
                });
                pipelines.particles = Some(pair(
                    device,
                    "Particle",
                    &module,
                    "vs_particle",
                    &[
                        &pp.uniform_bind_group_layout,
                        &pp.instance_bind_group_layout,
                        &pp.texture_bind_group_layout,
                    ],
                    &[],
                    None,
                ));
            }
        }
        let (Some(pipelines), Some(resources)) =
            (&self.fog_veil.pipelines, &self.postprocess_resources)
        else {
            return false;
        };
        let Some(targets) = &resources.fog_veil else {
            return false;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Fog Veil Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.veil_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // No weight, and all of what's behind shows
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            // Read-only: the scene's depth, which nothing here writes
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // In the main pass's order: blended meshes, echoes, then particles,
        // each layer back to front, so the "over" weights stack as the
        // colours did.
        let mut particles_bound = false;
        for item in self.draws.drawn() {
            let Some(light) = veils(item) else {
                continue;
            };
            match &item.draw {
                Draw::Static(d) => {
                    pass.set_pipeline(pipelines.meshes.get(light));
                    pass.set_bind_group(0, &*d.transform_bind_group, &[]);
                    pass.set_bind_group(1, &*d.material_bind_group, &[]);
                    pass.set_bind_group(2, &self.lights.bind_group, &[]);
                    pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
                    pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.index_count, 0, 0..1);
                }
                Draw::Skinned(d) => {
                    pass.set_pipeline(pipelines.skinned.get(light));
                    pass.set_bind_group(0, &*d.transform_bind_group, &[]);
                    pass.set_bind_group(1, &*d.material_bind_group, &[]);
                    pass.set_bind_group(2, &self.lights.bind_group, &[]);
                    pass.set_bind_group(3, &*d.bone_bind_group, &[]);
                    pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
                    pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.index_count, 0, 0..1);
                }
                Draw::Echo(d) => {
                    pass.set_pipeline(pipelines.echoes.get(light));
                    pass.set_bind_group(0, &*d.transform_bind_group, &[]);
                    pass.set_bind_group(1, &*d.material_bind_group, &[]);
                    pass.set_bind_group(2, &self.lights.bind_group, &[]);
                    pass.set_bind_group(3, &*d.bone_bind_group, &[]);
                    pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
                    pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.index_count, 0, 0..1);
                }
                Draw::Particles(n) => {
                    let (Some(pair), Some(buffers), Some(draw)) = (
                        &pipelines.particles,
                        &self.particle_buffers,
                        self.particle_draws.get(*n as usize),
                    ) else {
                        continue;
                    };
                    if !particles_bound {
                        pass.set_index_buffer(
                            buffers.quad_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.set_bind_group(0, &buffers.uniform_bind_group, &[]);
                        pass.set_bind_group(1, buffers.instance_bind_group(), &[]);
                        particles_bound = true;
                    }
                    pass.set_pipeline(pair.get(light));
                    pass.set_bind_group(2, draw.texture_bind_group.as_ref(), &[]);
                    let first = draw.first_instance;
                    pass.draw_indexed(0..6, 0, first..first + draw.instance_count);
                }
                Draw::Billboard(_) | Draw::Terrain(_) => {}
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::FOG_VEIL_BLEND;

    /// The blend on one pixel, as the GPU does it.
    fn over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
        let c = FOG_VEIL_BLEND.color;
        let a = FOG_VEIL_BLEND.alpha;
        assert_eq!(c.src_factor, wgpu::BlendFactor::One);
        assert_eq!(c.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(a.src_factor, wgpu::BlendFactor::Zero);
        assert_eq!(a.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        let k = 1.0 - src[3];
        [
            src[0] + dst[0] * k,
            src[1] + dst[1] * k,
            src[2] + dst[2] * k,
            dst[3] * k,
        ]
    }

    #[test]
    fn layers_stack_their_weights_and_what_shows_behind() {
        let clear = [0.0, 0.0, 0.0, 1.0];
        // One layer at depth 0.9 with alpha 0.25: its depth comes back
        // exactly, and three quarters of what's behind shows
        let one = over(clear, [0.25 * 0.1, 0.25, 0.0, 0.25]);
        assert!((1.0 - one[0] / one[1] - 0.9).abs() < 1e-6);
        assert!((one[3] - 0.75).abs() < 1e-6);
        // A light-only layer in front adds weight and hides nothing
        let lit = over(one, [0.5 * 0.2, 0.5, 0.0, 0.0]);
        assert!((lit[3] - 0.75).abs() < 1e-6);
        let depth = 1.0 - lit[0] / lit[1];
        assert!(depth > 0.8 && depth < 0.9, "between the two: {depth}");
        // An opaque layer in front hides the rest's weight entirely
        let full = over(lit, [0.5, 1.0, 0.0, 1.0]);
        assert!((1.0 - full[0] / full[1] - 0.5).abs() < 1e-6);
        assert_eq!(full[3], 0.0);
    }
}
