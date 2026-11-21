//! A style's `blend` (#369): how much of its result shows over what it
//! read, the key every style's table takes (`[post_process.styles.<name>]
//! blend = 0.4`, or a script's `set_post_style_param(name, "blend", t)`).
//!
//! The stage draws it, not the style: after a style whose blend is under 1
//! has drawn, [`BlendPass::draw`] draws the style's input back over its
//! output with the blend constant `1 - blend`, so no style needs a mix of
//! its own. That is one full-screen draw, only while the blend is under 1,
//! and nothing is allocated per frame: one pipeline per target format,
//! made the first time it's needed, and the input's bind group is kept
//! with the chain like every style's.
//!
//! A style whose table says `blend_source = "tear"` (#746) shows only where
//! the frame tears: [`TearBlend`] puts its input back everywhere else, by
//! the same mask a torn prop's blend follows (#397). That is how the old
//! render modes' looks (matrix, blood, tron) tear into the frame now.

use super::{kit, StyleFrame, StyleInput};
use crate::gpu_counts::{CountedDevice, CountedQueue};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex, OnceLock};

const BIND: &str = "style blend input";
const TEAR_BIND: &str = "style tear blend input";

/// The blend draw's pipelines, one per target format, made on first use,
/// the reveal's (#406), which draws in its place when the style has one,
/// and the tear's (#746), for a style that follows the tear.
#[derive(Default)]
pub struct BlendPass {
    shared: OnceLock<(kit::Shader, wgpu::BindGroupLayout)>,
    pipelines: Mutex<Vec<(wgpu::TextureFormat, Arc<wgpu::RenderPipeline>)>>,
    reveal: super::reveal::RevealPass,
    tear: TearBlend,
}

/// The tear blend's uniform, laid out as `TearBlend` in blend_tear.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TearUniform {
    tear: [f32; 4],
    blend: f32,
    aspect: f32,
    _pad: [f32; 2],
}

/// A uniform slot per style that follows the tear in a frame; past this
/// the rest draw untorn, with a warning.
const TEAR_SLOTS: u64 = 16;
const TEAR_SLOT: u64 = 256;

struct TearShared {
    module: kit::Shader,
    input_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
}

/// The draw that shows a style only inside the frame's tear (#746): one
/// full-screen draw writing the style's input with alpha `1 - blend x
/// mask` over its output. Made on first use; uniforms in
/// [`TEAR_SLOTS`] dynamic-offset slots of one buffer, reset each frame.
#[derive(Default)]
pub struct TearBlend {
    shared: OnceLock<TearShared>,
    pipelines: Mutex<Vec<(wgpu::TextureFormat, Arc<wgpu::RenderPipeline>)>>,
    /// (frame, next free slot)
    slots: Mutex<(u64, u64)>,
    warned: std::sync::atomic::AtomicBool,
}

impl TearBlend {
    fn shared(&self, device: &wgpu::Device) -> &TearShared {
        self.shared.get_or_init(|| {
            let module = kit::shader(
                device,
                "Style Tear Blend Shader",
                kit::wgsl!("../tear_mask.wgsl", "blend_tear.wgsl"),
            );
            let input_layout = kit::sampled_layout(device, "Style Tear Blend BGL");
            let size = wgpu::BufferSize::new(std::mem::size_of::<TearUniform>() as u64);
            let uniform_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Style Tear Blend Uniform BGL"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: size,
                        },
                        count: None,
                    }],
                });
            let uniform = device.create_buffer_counted(&wgpu::BufferDescriptor {
                label: Some("Style Tear Blend Uniforms"),
                size: TEAR_SLOTS * TEAR_SLOT,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform_group = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Style Tear Blend Uniform Group"),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform,
                        offset: 0,
                        size,
                    }),
                }],
            });
            TearShared {
                module,
                input_layout,
                uniform_layout,
                uniform,
                uniform_group,
            }
        })
    }

    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<wgpu::RenderPipeline> {
        let shared = self.shared(device);
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, p)) = pipelines.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        // The input, by the alpha written, over the style's result; the
        // target's own alpha kept
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline: Arc<wgpu::RenderPipeline> =
            crate::pipeline_cache::in_scope(("Style Tear Blend", format), || {
                kit::fullscreen_pipeline_with(
                    device,
                    "Style Tear Blend Pipeline",
                    &shared.module,
                    "fs_main",
                    &[&shared.input_layout, &shared.uniform_layout],
                    format,
                    Some(blend),
                )
            });
        pipelines.push((format, pipeline.clone()));
        pipeline
    }

    fn slot(&self, frame: u64) -> Option<u64> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.0 != frame {
            *slots = (frame, 0);
        }
        let slot = slots.1;
        if slot >= TEAR_SLOTS {
            return None;
        }
        slots.1 += 1;
        Some(slot)
    }

    /// Show the style just drawn into `output` only inside the frame's
    /// tear, `blend` of it there: `input` back over it everywhere else.
    /// `false` when it drew nothing (no slot left).
    pub fn draw(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
        blend: f32,
    ) -> bool {
        let Some(slot) = self.slot(frame.frame) else {
            if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(
                    "more than {TEAR_SLOTS} styles follow the tear in one frame; the rest draw untorn"
                );
            }
            return false;
        };
        let uniform = TearUniform {
            tear: crate::postprocess::tear_params(frame.config, frame.time),
            blend: blend.clamp(0.0, 1.0),
            aspect: frame.width.max(1) as f32 / frame.height.max(1) as f32,
            _pad: [0.0; 2],
        };
        let pipeline = self.pipeline(frame.device, frame.format);
        let shared = self.shared(frame.device);
        frame.queue.write_buffer_counted(
            &shared.uniform,
            slot * TEAR_SLOT,
            bytemuck::bytes_of(&uniform),
        );
        let group = frame.sampled_input(TEAR_BIND, &shared.input_layout, input);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Tear Blend Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*group, &[]);
        pass.set_bind_group(1, &shared.uniform_group, &[(slot * TEAR_SLOT) as u32]);
        pass.draw(0..3, 0..1);
        true
    }
}

impl BlendPass {
    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (Arc<wgpu::RenderPipeline>, &wgpu::BindGroupLayout) {
        let (module, layout) = self.shared.get_or_init(|| {
            (
                kit::shader(device, "Style Blend Shader", kit::wgsl!("blend.wgsl")),
                kit::sampled_layout(device, "Style Blend BGL"),
            )
        });
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, p)) = pipelines.iter().find(|(f, _)| *f == format) {
            return (p.clone(), layout);
        }
        let component = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline: Arc<wgpu::RenderPipeline> =
            crate::pipeline_cache::in_scope(("Style Blend", format), || {
                kit::fullscreen_pipeline_with(
                    device,
                    "Style Blend Pipeline",
                    module,
                    "fs_main",
                    &[layout],
                    format,
                    Some(wgpu::BlendState {
                        color: component,
                        alpha: component,
                    }),
                )
            });
        pipelines.push((format, pipeline.clone()));
        (pipeline, layout)
    }

    /// Show only `blend` of what was just drawn into `output`: draw
    /// `input` back over it with weight `1 - blend`. Nothing to do at 1.
    /// A style with a reveal between 0 and 1 (#406) shows only inside its
    /// region instead, `blend` of it there, with the edge's ink.
    pub fn draw(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
        blend: f32,
    ) {
        // Only where the frame tears (#746); a reveal is not drawn with it
        if frame.config.styles.follows_tear(frame.descriptor.name)
            && self.tear.draw(frame, encoder, input, output, blend)
        {
            return;
        }
        if let Some(reveal) = frame.config.styles.reveal(frame.descriptor.name) {
            let amount = reveal.amount.unwrap_or(1.0);
            if amount < 1.0
                && self
                    .reveal
                    .draw(frame, encoder, input, output, reveal, blend)
            {
                return;
            }
        }
        if blend >= 1.0 {
            return;
        }
        let keep = f64::from(1.0 - blend.max(0.0));
        let (pipeline, layout) = self.pipeline(frame.device, frame.format);
        let group = frame.sampled_input(BIND, layout, input);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Blend Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*group, &[]);
        pass.set_blend_constant(wgpu::Color {
            r: keep,
            g: keep,
            b: keep,
            a: keep,
        });
        pass.draw(0..3, 0..1);
    }
}
