//! Shared geometry edges (#301): one edge texture a frame, for every style
//! that asks for it.
//!
//! The ASCII spike (#261, round 2) found that edges from the geometry give
//! objects their definition: Sobels over the relative view distance and
//! over normals rebuilt from depth, near side only. Pencil, ASCII and
//! whatever comes next all want them, and each computing its own would pay
//! for the 5x5 unprojection once per style. A style says
//! `needs: &[StyleNeed::Edges]` in its descriptor instead; while any style
//! that does is on, the framework draws this pass once, before the first
//! stage runs, and every style reads the result from
//! [`StyleFrame::edges`](super::StyleFrame::edges).
//!
//! The texture is the depth's size (the scene's, under a render scale), so
//! an edge is one depth pixel wide; a display-stage style reads it by UV,
//! like the depth and the mask. `Rgba16Float`:
//!
//! | channel | what |
//! |---|---|
//! | `r` | silhouette strength: the relative-distance Sobel over [`PostProcessConfig::edge_depth`], near side only |
//! | `g` | crease strength: the rebuilt-normal Sobel over [`PostProcessConfig::edge_normal`], where the patch is one surface |
//! | `ba` | the unit gradient across the edge, screen space (x right, y down): the silhouette's where `r >= 1`, else the crease's |
//!
//! A value of 1 is an edge at the shared thresholds. The thresholds are
//! shared because the pass runs once; a style that wants more or fewer
//! lines compares the strengths against a level of its own (`r * k >= 1`).
//! The two pixels at each border are 0: there the patch clamps and the
//! repeated positions make a degenerate normal.
//!
//! [`PostProcessConfig::edge_depth`]: crate::postprocess::PostProcessConfig::edge_depth
//! [`PostProcessConfig::edge_normal`]: crate::postprocess::PostProcessConfig::edge_normal

use super::kit;
use super::targets::StyleTexture;
use super::StyleFrame;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;

/// The edge texture's format.
pub const EDGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The edge texture's name among the chain's targets.
pub(crate) const EDGE_TARGET: &str = "style edges";

/// What `--gpu-time` calls the pass.
pub const EDGE_LABEL: &str = "Style edges";

/// The thresholds' floor, so a slider at 0 can't divide by it.
const MIN_THRESHOLD: f32 = 1e-3;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgeUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    depth_threshold: f32,
    normal_threshold: f32,
    _pad: [f32; 3],
}

/// The pipeline, made the first time a style needs edges.
pub(crate) struct EdgePass {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
}

impl EdgePass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let uniform_bgl = kit::uniform_layout(device, "Style Edges Uniform BGL");
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Edges Depth BGL"),
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
        let pipeline = kit::fullscreen_pipeline(
            device,
            "Style Edges Pipeline",
            &kit::shader(device, "Style Edges Shader", kit::wgsl!("edges.wgsl")),
            &[&uniform_bgl, &depth_bgl],
            EDGE_FORMAT,
        );
        Self {
            pipeline,
            uniforms: kit::uniform_buffer::<EdgeUniforms>(device, "Style Edges Uniforms"),
            uniform_bgl,
            depth_bgl,
        }
    }

    /// Draw this frame's edges from `frame`'s depth and camera into the
    /// chain's edge target, and return it.
    pub(crate) fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Arc<StyleTexture> {
        let [w, h] = frame.depth_size;
        let target = frame
            .targets
            .get(frame.device, EDGE_TARGET, EDGE_FORMAT, w, h);
        let uniforms = EdgeUniforms {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            depth_threshold: frame.config.edge_depth.max(MIN_THRESHOLD),
            normal_threshold: frame.config.edge_normal.max(MIN_THRESHOLD),
            _pad: [0.0; 3],
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        let uniform_bg = frame.uniform("Style Edges Uniform BG", &self.uniform_bgl, &self.uniforms);
        let depth_bg = frame.depth_bind("Style Edges Depth BG", 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Style Edges Depth BG"),
                layout: &self.depth_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(frame.depth),
                }],
            })
        });
        kit::fullscreen_pass(
            encoder,
            "Style Edges Pass",
            &target.view,
            &self.pipeline,
            &[&uniform_bg, &depth_bg],
        );
        target
    }
}
