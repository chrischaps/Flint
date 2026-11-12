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

use super::{kit, StyleFrame, StyleInput};
use std::sync::{Arc, Mutex, OnceLock};

const BIND: &str = "style blend input";

/// The blend draw's pipelines, one per target format, made on first use.
#[derive(Default)]
pub struct BlendPass {
    shared: OnceLock<(wgpu::ShaderModule, wgpu::BindGroupLayout)>,
    pipelines: Mutex<Vec<(wgpu::TextureFormat, Arc<wgpu::RenderPipeline>)>>,
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
        let pipeline = Arc::new(kit::fullscreen_pipeline_with(
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
        ));
        pipelines.push((format, pipeline.clone()));
        (pipeline, layout)
    }

    /// Show only `blend` of what was just drawn into `output`: draw
    /// `input` back over it with weight `1 - blend`. Nothing to do at 1.
    pub fn draw(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
        blend: f32,
    ) {
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
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
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
