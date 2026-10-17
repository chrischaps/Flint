//! Depth-tested world-space lines (ADR 0083).
//!
//! Scripts queue `WorldLine`s with `draw_world_line`. Each frame the renderer
//! projects them with the camera it renders with and expands every segment
//! into a screen-space quad of the requested width. It then draws them onto
//! the finished, tonemapped image, before the HUD. The scene's resolved depth
//! hides whatever lies behind geometry. The lines don't write depth, so they
//! never hide each other or anything drawn later.
//!
//! The width is in logical points, as the HUD's lines are, and the edges are
//! anti-aliased in the fragment shader, since the pass runs after MSAA and
//! FXAA. Colours are display-space (sRGB), as the HUD's are, so a ring keeps
//! the colour it had as a HUD line. Fog, bloom and tonemapping don't touch it.

use bytemuck::{Pod, Zeroable};
use flint_core::world_line::WorldLine;

/// How far each end is pulled toward the eye, as a fraction of its distance.
/// A line laid on a surface would otherwise fight that surface for depth. At
/// 100 m this is 20 cm, which keeps a ring on the ground visible and still
/// hides it behind anything that really stands in front.
pub const DEPTH_SLACK: f32 = 0.002;

/// Anti-aliasing feather, in pixels, added to each side of a line.
const FEATHER_PX: f32 = 1.0;

/// One vertex of a line's quad. Layout must match `VsIn` in
/// `world_line_shader.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct WorldLineVertex {
    /// NDC position with `w = 1`. NDC depth is affine across the screen, so
    /// the quad interpolates the segment's true depth.
    pub position: [f32; 4],
    /// RGBA in the target's space (linear for an sRGB target).
    pub color: [f32; 4],
    /// `x`: signed distance from the centre line in pixels; `y`: half the
    /// line's width in pixels.
    pub edge: [f32; 2],
}

impl WorldLineVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x2];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// The camera and target a frame's lines are built for.
#[derive(Debug, Clone, Copy)]
pub struct LineView {
    /// Column-major view-projection (`m[col][row]`), wgpu clip space.
    pub view_proj: [[f32; 4]; 4],
    /// Camera position in world space.
    pub eye: [f32; 3],
    /// Target size in physical pixels.
    pub viewport: [f32; 2],
    /// Physical pixels per logical point.
    pub px_per_point: f32,
    /// Convert display-space colours to linear (an sRGB target re-encodes).
    pub linear_output: bool,
}

/// Expand `lines` into a triangle list, six vertices a segment, appended to
/// `out`. A segment wholly behind the near plane, or shorter than a
/// hundredth of a pixel on screen, adds nothing. One crossing the near plane
/// is cut there.
pub fn build_vertices(lines: &[WorldLine], view: &LineView, out: &mut Vec<WorldLineVertex>) {
    let [w, h] = view.viewport;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    for line in lines {
        let half = line.thickness.max(0.0) * view.px_per_point * 0.5;
        if half <= 0.0 || line.color[3] <= 0.0 {
            continue;
        }
        let Some((p, q)) = clip_segment(
            project(&view.view_proj, toward(line.a, view.eye)),
            project(&view.view_proj, toward(line.b, view.eye)),
        ) else {
            continue;
        };
        let (pn, qn) = (ndc(p), ndc(q));
        // Pixel space, y up (NDC's direction; only lengths matter here).
        let pa = [(pn[0] * 0.5 + 0.5) * w, (pn[1] * 0.5 + 0.5) * h];
        let pb = [(qn[0] * 0.5 + 0.5) * w, (qn[1] * 0.5 + 0.5) * h];
        let d = [pb[0] - pa[0], pb[1] - pa[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 0.01 {
            continue;
        }
        let u = [d[0] / len, d[1] / len];
        let n = [-u[1], u[0]];
        let e = half + FEATHER_PX;
        // Butt caps, half a pixel long, so a polyline's joints meet.
        let cap = 0.5;
        let to_ndc = [2.0 / w, 2.0 / h];
        let corner = |at: [f32; 3], along: f32, side: f32| -> [f32; 4] {
            [
                at[0] + (u[0] * along + n[0] * side) * to_ndc[0],
                at[1] + (u[1] * along + n[1] * side) * to_ndc[1],
                at[2],
                1.0,
            ]
        };
        let color = if view.linear_output {
            [
                srgb_to_linear(line.color[0]),
                srgb_to_linear(line.color[1]),
                srgb_to_linear(line.color[2]),
                line.color[3],
            ]
        } else {
            line.color
        };
        let v = |position: [f32; 4], side: f32| WorldLineVertex {
            position,
            color,
            edge: [side, half],
        };
        let a0 = v(corner(pn, -cap, -e), -e);
        let a1 = v(corner(pn, -cap, e), e);
        let b0 = v(corner(qn, cap, -e), -e);
        let b1 = v(corner(qn, cap, e), e);
        out.extend_from_slice(&[a0, b0, b1, a0, b1, a1]);
    }
}

/// `p` moved toward `eye` by `DEPTH_SLACK` of the distance between them.
fn toward(p: [f32; 3], eye: [f32; 3]) -> [f32; 3] {
    [
        p[0] + (eye[0] - p[0]) * DEPTH_SLACK,
        p[1] + (eye[1] - p[1]) * DEPTH_SLACK,
        p[2] + (eye[2] - p[2]) * DEPTH_SLACK,
    ]
}

/// World point to clip space with a column-major view-projection.
fn project(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (row, o) in out.iter_mut().enumerate() {
        *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
    }
    out
}

/// Cut a clip-space segment at the near plane (`z = 0` in wgpu clip space,
/// for perspective and orthographic cameras alike).
fn clip_segment(p: [f32; 4], q: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    match (p[2] >= 0.0, q[2] >= 0.0) {
        (true, true) => Some((p, q)),
        (false, false) => None,
        (p_in, _) => {
            let t = p[2] / (p[2] - q[2]);
            let mut m = [0.0f32; 4];
            for i in 0..4 {
                m[i] = p[i] + (q[i] - p[i]) * t;
            }
            m[2] = 0.0;
            if m[3] <= 0.0 {
                return None;
            }
            Some(if p_in { (p, m) } else { (m, q) })
        }
    }
}

fn ndc(c: [f32; 4]) -> [f32; 3] {
    [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The pipeline and a vertex buffer that grows to the busiest frame.
pub struct WorldLinePipeline {
    pipeline: wgpu::RenderPipeline,
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    vertices: Vec<WorldLineVertex>,
}

impl WorldLinePipeline {
    /// `format` is the final target's (the surface's); the pass is single
    /// sample against a `Depth32Float` depth view.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("World Line Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world_line_shader.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("World Line Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("World Line Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_world_line"),
                buffers: &[WorldLineVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_world_line"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            buffer: None,
            capacity: 0,
            vertices: Vec::new(),
        }
    }

    /// Build and draw `lines` onto `target`, tested against `depth`. Records
    /// nothing when no segment survives projection.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        lines: &[WorldLine],
        view: &LineView,
    ) {
        self.vertices.clear();
        build_vertices(lines, view, &mut self.vertices);
        if self.vertices.is_empty() {
            return;
        }
        if self.capacity < self.vertices.len() {
            let capacity = self.vertices.len().next_power_of_two();
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("World Line Vertices"),
                size: (capacity * std::mem::size_of::<WorldLineVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.capacity = capacity;
        }
        let buffer = self.buffer.as_ref().expect("allocated above");
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.vertices));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("World Lines"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..self.vertices.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera at the origin looking down -z: 90° vertical fov, square
    /// viewport, near 1, far 100 (wgpu depth 0..1). Column-major.
    fn perspective() -> [[f32; 4]; 4] {
        let (n, f) = (1.0f32, 100.0f32);
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, f / (n - f), -1.0],
            [0.0, 0.0, n * f / (n - f), 0.0],
        ]
    }

    fn view() -> LineView {
        LineView {
            view_proj: perspective(),
            eye: [0.0; 3],
            viewport: [200.0, 200.0],
            px_per_point: 1.0,
            linear_output: false,
        }
    }

    fn line(a: [f32; 3], b: [f32; 3], thickness: f32) -> WorldLine {
        WorldLine {
            a,
            b,
            color: [1.0, 0.5, 0.25, 0.8],
            thickness,
        }
    }

    fn build(lines: &[WorldLine], v: &LineView) -> Vec<WorldLineVertex> {
        let mut out = Vec::new();
        build_vertices(lines, v, &mut out);
        out
    }

    #[test]
    fn a_segment_is_two_triangles_as_wide_as_asked_plus_the_feather() {
        // Horizontal across the view at z = -10: x from -5 to 5 lands at
        // NDC -0.5..0.5, so pixels 50..150 on a 200 px viewport.
        let out = build(&[line([-5.0, 0.0, -10.0], [5.0, 0.0, -10.0], 4.0)], &view());
        assert_eq!(out.len(), 6);
        let ys: Vec<f32> = out.iter().map(|v| v.position[1] * 100.0).collect();
        let span = ys.iter().cloned().fold(f32::MIN, f32::max)
            - ys.iter().cloned().fold(f32::MAX, f32::min);
        // 4 px wide plus a 1 px feather each side.
        assert!((span - 6.0).abs() < 1e-3, "span {span}");
        for v in &out {
            assert_eq!(v.edge[1], 2.0);
            assert_eq!(v.edge[0].abs(), 3.0);
            assert_eq!(v.position[3], 1.0);
            assert_eq!(v.color, [1.0, 0.5, 0.25, 0.8]);
        }
    }

    #[test]
    fn width_is_in_logical_points() {
        let mut v = view();
        v.px_per_point = 1.5;
        let out = build(&[line([-5.0, 0.0, -10.0], [5.0, 0.0, -10.0], 2.0)], &v);
        assert_eq!(out[0].edge[1], 1.5);
    }

    #[test]
    fn depth_is_the_segments_own_pulled_slightly_toward_the_eye() {
        let out = build(&[line([0.0, -5.0, -10.0], [0.0, 5.0, -50.0], 2.0)], &view());
        let p = perspective();
        let true_near = ndc(project(&p, [0.0, -5.0, -10.0]))[2];
        let true_far = ndc(project(&p, [0.0, 5.0, -50.0]))[2];
        let (zn, zf) = (out[0].position[2], out[1].position[2]);
        assert!(zn < true_near && true_near - zn < 1e-3, "{zn} vs {true_near}");
        assert!(zf < true_far && true_far - zf < 1e-3, "{zf} vs {true_far}");
    }

    #[test]
    fn behind_the_camera_draws_nothing_and_crossing_it_is_cut() {
        assert!(build(&[line([0.0, 0.0, 5.0], [1.0, 0.0, 9.0], 2.0)], &view()).is_empty());
        let out = build(&[line([0.0, -1.0, -10.0], [0.0, -1.0, 10.0], 2.0)], &view());
        assert_eq!(out.len(), 6);
        for v in &out {
            assert!(v.position[2] >= 0.0 && v.position[2].is_finite());
            assert!(v.position[0].is_finite() && v.position[1].is_finite());
        }
    }

    #[test]
    fn invisible_and_degenerate_lines_are_skipped() {
        let mut clear = line([-5.0, 0.0, -10.0], [5.0, 0.0, -10.0], 2.0);
        clear.color[3] = 0.0;
        let dot = line([1.0, 1.0, -10.0], [1.0, 1.0, -10.0], 2.0);
        let hairless = line([-5.0, 0.0, -10.0], [5.0, 0.0, -10.0], 0.0);
        assert!(build(&[clear, dot, hairless], &view()).is_empty());
    }

    #[test]
    fn an_srgb_target_gets_linear_colour_and_the_same_alpha() {
        let mut v = view();
        v.linear_output = true;
        let out = build(&[line([-5.0, 0.0, -10.0], [5.0, 0.0, -10.0], 2.0)], &v);
        assert_eq!(out[0].color[0], 1.0);
        assert!((out[0].color[1] - 0.214).abs() < 1e-3);
        assert_eq!(out[0].color[3], 0.8);
    }

    #[test]
    fn vertex_layout_matches_the_shader() {
        assert_eq!(std::mem::size_of::<WorldLineVertex>(), 40);
    }
}
