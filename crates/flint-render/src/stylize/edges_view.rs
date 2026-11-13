//! Edges view (#301): the shared geometry edge texture, made visible. The
//! scene fades to grey and every edge pixel takes a hue from its
//! direction; silhouettes are solid, creases a little softer, and
//! strengths just under the threshold show as a faint ghost, which is
//! what tuning `edge_depth` / `edge_normal` wants to see. A scene switches
//! it on like any style:
//!
//! ```toml
//! [post_process]
//! edge_depth = 0.6     # the shared thresholds every edge-reading style sees
//! edge_normal = 2.5
//!
//! [post_process.styles.edges_view]
//! dim = 0.3
//! ```
//!
//! It is also the example for a style that reads the shared edges:
//! `needs: &[StyleNeed::Edges]`, and [`StyleFrame::edges_bind`] for the
//! bind group.

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleNeed, StyleParam,
    StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("edges_view", "Edges view", StyleStage::Display, create)
        // A debug view of the whole frame (#384)
        .draws_over_entities()
        .params(&[
            StyleParam::new("dim", 0.3, 0.0..=1.0),
            StyleParam::new("threshold", 1.0, 0.1..=4.0),
            StyleParam::new("ghost", 0.5, 0.0..=1.0),
        ])
        .needs(&[StyleNeed::Edges])
        .hints(&[
            StyleHint::new(
                "dim",
                "Scene dim",
                "Brightness of the grey scene under the edges",
            ),
            StyleHint::new(
                "threshold",
                "Threshold",
                "Strength that counts as an edge; 1 is the shared edge_depth / edge_normal",
            ),
            StyleHint::new(
                "ghost",
                "Near misses",
                "How much a strength under the threshold shows",
            ),
        ]);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Edges View Uniform BGL");
    let input_bgl = kit::sampled_layout(device, "Edges View Input BGL");
    let edges_bgl = kit::texture_layout(device, "Edges View Edges BGL");
    Box::new(EdgesView {
        pipeline: kit::fullscreen_pipeline(
            device,
            "Edges View Pipeline",
            &kit::shader(device, "Edges View Shader", kit::wgsl!("edges_view.wgsl")),
            &[&uniform_bgl, &input_bgl, &edges_bgl],
            d.format,
        ),
        uniforms: kit::uniform_buffer::<EdgesViewUniforms>(device, "Edges View Uniforms"),
        uniform_bgl,
        input_bgl,
        edges_bgl,
    })
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgesViewUniforms {
    dim: f32,
    threshold: f32,
    ghost: f32,
    srgb: f32,
}

struct EdgesView {
    pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    edges_bgl: wgpu::BindGroupLayout,
}

impl StylePass for EdgesView {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        // The framework draws the edges whenever a style that needs them is
        // on (the same contract as mask_view's mask)
        let Some(edges) = frame.edges_bind("Edges View Edges BG", &self.edges_bgl) else {
            return;
        };
        let input_bg = frame.sampled_input("Edges View Input BG", &self.input_bgl, input);
        let uniforms = EdgesViewUniforms {
            dim: frame.param("dim").clamp(0.0, 1.0),
            threshold: frame.param("threshold").clamp(0.1, 4.0),
            ghost: frame.param("ghost").clamp(0.0, 1.0),
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        kit::fullscreen_pass(
            encoder,
            "Edges View Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Edges View Uniform BG", &self.uniform_bgl, &self.uniforms),
                &input_bg,
                &edges,
            ],
        );
    }
}
