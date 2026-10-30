//! Depth-tested world text (ADR 0104).
//!
//! Scripts queue `WorldText`s with `draw_world_text`. Each frame the player
//! lays them out with egui's fonts, the HUD's own, inside the egui pass, so
//! any glyph they need reaches the font atlas with that pass's upload. After
//! the pass each label is tessellated on its own and its vertices are moved
//! into the 3D frame: a billboard keeps its screen layout at its anchor's
//! depth, and text on a plane is laid in that plane and projected corner by
//! corner. They're drawn onto the HUD's target before the HUD, against the
//! scene's resolved depth (`LessEqual`, no depth write), sampling egui's
//! font atlas. Colours, strokes and shadows are the HUD's, and so is the
//! blend, so a name looks the same as it would drawn with `draw_text_opts`.
//! Fog, bloom and tonemapping don't touch it.

use super::fonts::{resolve_font_id, text_layout_job};
use super::hud_render::to_color32;
use bytemuck::{Pod, Zeroable};
use flint_core::world_text::WorldText;
use flint_render::world_line_pipeline::DEPTH_SLACK;
use std::collections::HashSet;
use std::ops::Range;

/// The camera and screen a frame's text is laid out for.
#[derive(Debug, Clone, Copy)]
pub(super) struct TextView {
    /// Column-major view-projection (`m[col][row]`), wgpu clip space.
    pub view_proj: [[f32; 4]; 4],
    /// Camera position in world space.
    pub eye: [f32; 3],
    /// Screen size in logical points.
    pub screen: [f32; 2],
    /// Physical pixels per logical point.
    pub px_per_point: f32,
}

/// Text on a plane is rasterised at the size its em spans on screen, in
/// whole pixels, held to this range. Whole pixels keep the font atlas to a
/// few sizes as the camera moves.
const PLANE_EM_PX: (f32, f32) = (4.0, 128.0);

/// Where a label's layout, in points with its anchor at the origin and y
/// down, goes in the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    /// Facing the screen: `origin` is the anchor in NDC, `per_point` NDC
    /// units per point, `depth` the anchor's NDC depth.
    Screen {
        origin: [f32; 2],
        per_point: [f32; 2],
        depth: f32,
    },
    /// In a plane: a point is `at + right·x·k − up·y·k`, projected.
    Plane {
        at: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        k: f32,
    },
}

/// A label laid out, waiting for the tessellator.
pub(super) struct Laid {
    shapes: Vec<egui::Shape>,
    place: Place,
}

/// One vertex of a glyph quad. Layout must match `VsIn` in
/// `world_text_shader.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub(super) struct GlyphVertex {
    /// Clip-space position. A billboard's has `w = 1`; a plane's keeps its
    /// true `w`, so the glyphs are sampled in perspective.
    pub position: [f32; 4],
    /// Normalised font-atlas coordinates.
    pub uv: [f32; 2],
    /// Premultiplied sRGBA, packed as egui's `Color32`.
    pub color: u32,
}

impl GlyphVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Uint32];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// A frame's text, ready to draw: one triangle list, in runs by texture.
#[derive(Default)]
pub(super) struct GlyphMesh {
    pub vertices: Vec<GlyphVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<(egui::TextureId, Range<u32>)>,
}

impl GlyphMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Lay out `texts` for `view`. Run inside the egui pass, so the glyphs this
/// asks for are uploaded with it. Labels behind the camera, past the far
/// plane or seen from behind their plane are dropped here.
pub(super) fn lay_out(
    ctx: &egui::Context,
    texts: &[WorldText],
    view: &TextView,
    fonts: &HashSet<String>,
    warned: &mut HashSet<String>,
) -> Vec<Laid> {
    let mut out = Vec::new();
    for t in texts {
        if t.text.is_empty() || t.color[3] <= 0.0 || t.size <= 0.0 {
            continue;
        }
        let Some((place, points, scale)) = place(t, view) else {
            continue;
        };
        let font_id = resolve_font_id(points, t.font.as_deref(), fonts, warned);
        let galley = |color: egui::Color32| {
            ctx.fonts(|f| {
                f.layout_job(text_layout_job(
                    &t.text,
                    font_id.clone(),
                    color,
                    t.spacing * scale,
                ))
            })
        };
        let main = to_color32(&t.color);
        let body = galley(main);
        let size = body.size();
        let pos = egui::pos2(
            -size.x * f32::from(t.align.min(2)) * 0.5,
            -size.y * f32::from(t.valign.min(2)) * 0.5,
        );
        // The HUD's order: shadow, then the stroke's eight offsets, then
        // the text itself.
        let mut shapes = Vec::new();
        if let Some((color, dx, dy)) = t.shadow {
            let c = to_color32(&color);
            shapes.push(egui::Shape::galley(pos + egui::vec2(dx, dy) * scale, galley(c), c));
        }
        if let Some((color, width)) = t.stroke {
            let c = to_color32(&color);
            let stroke = galley(c);
            let w = width * scale;
            for (dx, dy) in [(-w, 0.0), (w, 0.0), (0.0, -w), (0.0, w), (-w, -w), (w, -w), (-w, w), (w, w)] {
                shapes.push(egui::Shape::galley(pos + egui::vec2(dx, dy), stroke.clone(), c));
            }
        }
        shapes.push(egui::Shape::galley(pos, body, main));
        out.push(Laid { shapes, place });
    }
    out
}

/// Where `t` goes, the font size to lay it out at in points, and how many
/// points one of its own length units is (1 for a billboard).
fn place(t: &WorldText, view: &TextView) -> Option<(Place, f32, f32)> {
    let [w, h] = view.screen;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    match t.plane {
        None => {
            let c = project(&view.view_proj, toward(t.at, view.eye));
            if c[3] <= 0.0 || c[2] < 0.0 || c[2] > c[3] {
                return None;
            }
            let ndc = [c[0] / c[3], c[1] / c[3], c[2] / c[3]];
            // Snap the anchor to a whole pixel, so the glyphs land on the
            // grid as HUD text does.
            let px = [w * view.px_per_point, h * view.px_per_point];
            let snap = |n: f32, size: f32| ((n * 0.5 + 0.5) * size).round() / size * 2.0 - 1.0;
            Some((
                Place::Screen {
                    origin: [snap(ndc[0], px[0]), snap(ndc[1], px[1])],
                    per_point: [2.0 / w, -2.0 / h],
                    depth: ndc[2],
                },
                t.size,
                1.0,
            ))
        }
        Some(plane) => {
            if !plane.faces(t.at, view.eye) {
                return None;
            }
            let (right, up) = plane.basis()?;
            let em_px = em_on_screen(t, right, up, view)?;
            let em_px = em_px.round().clamp(PLANE_EM_PX.0, PLANE_EM_PX.1);
            let points = em_px / view.px_per_point;
            Some((Place::Plane { at: t.at, right, up, k: t.size / points }, points, points / t.size))
        }
    }
}

/// How many pixels `t`'s em spans on screen at its anchor, along whichever
/// of its plane's axes is longer there.
fn em_on_screen(t: &WorldText, right: [f32; 3], up: [f32; 3], view: &TextView) -> Option<f32> {
    let px = |p: [f32; 3]| -> Option<[f32; 2]> {
        let c = project(&view.view_proj, p);
        (c[3] > 0.0).then(|| {
            [
                c[0] / c[3] * 0.5 * view.screen[0] * view.px_per_point,
                c[1] / c[3] * 0.5 * view.screen[1] * view.px_per_point,
            ]
        })
    };
    let a = px(t.at)?;
    let span = |d: [f32; 3]| -> Option<f32> {
        let b = px(add(t.at, d, t.size))?;
        Some(((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt())
    };
    Some(span(up)?.max(span(right)?))
}

/// Tessellate each label on its own and move its vertices into the frame.
/// Run after the egui pass, so texture coordinates are normalised against
/// the font atlas as it's uploaded. A label on a plane that reaches behind
/// the camera is dropped whole.
pub(super) fn tessellate(ctx: &egui::Context, laid: Vec<Laid>, view: &TextView) -> GlyphMesh {
    let mut mesh = GlyphMesh::default();
    let clip_rect = egui::Rect::from_min_max(egui::pos2(-1e7, -1e7), egui::pos2(1e7, 1e7));
    for label in laid {
        let shapes = label
            .shapes
            .into_iter()
            .map(|shape| egui::epaint::ClippedShape { clip_rect, shape })
            .collect();
        let start_v = mesh.vertices.len();
        let start_i = mesh.indices.len();
        let start_d = mesh.draws.len();
        let mut behind = false;
        for prim in ctx.tessellate(shapes, view.px_per_point) {
            let egui::epaint::Primitive::Mesh(m) = prim.primitive else {
                continue;
            };
            let base = mesh.vertices.len() as u32;
            for v in &m.vertices {
                let position = frame_position(&label.place, [v.pos.x, v.pos.y], view);
                behind |= position[3] <= 0.0 || position[2] < 0.0;
                mesh.vertices.push(GlyphVertex {
                    position,
                    uv: [v.uv.x, v.uv.y],
                    color: u32::from_le_bytes(v.color.to_array()),
                });
            }
            let first = mesh.indices.len() as u32;
            mesh.indices.extend(m.indices.iter().map(|i| base + i));
            let last = mesh.indices.len() as u32;
            match mesh.draws.last_mut() {
                Some((id, run)) if *id == m.texture_id && run.end == first => run.end = last,
                _ => mesh.draws.push((m.texture_id, first..last)),
            }
        }
        if behind {
            mesh.vertices.truncate(start_v);
            mesh.indices.truncate(start_i);
            mesh.draws.truncate(start_d);
            if let Some((_, run)) = mesh.draws.last_mut() {
                run.end = run.end.min(start_i as u32);
            }
        }
    }
    mesh
}

/// A layout point (points, anchor at the origin, y down) in clip space.
fn frame_position(place: &Place, p: [f32; 2], view: &TextView) -> [f32; 4] {
    match *place {
        Place::Screen { origin, per_point, depth } => {
            [origin[0] + p[0] * per_point[0], origin[1] + p[1] * per_point[1], depth, 1.0]
        }
        Place::Plane { at, right, up, k } => {
            let world = add(add(at, right, p[0] * k), up, -p[1] * k);
            project(&view.view_proj, toward(world, view.eye))
        }
    }
}

fn add(a: [f32; 3], d: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + d[0] * s, a[1] + d[1] * s, a[2] + d[2] * s]
}

/// `p` moved toward `eye` by the world lines' slack, so text painted on a
/// surface doesn't fight it for depth.
fn toward(p: [f32; 3], eye: [f32; 3]) -> [f32; 3] {
    add(p, [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]], DEPTH_SLACK)
}

/// World point to clip space with a column-major view-projection.
fn project(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (row, o) in out.iter_mut().enumerate() {
        *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
    }
    out
}

/// The pipeline and buffers that grow to the busiest frame.
pub(super) struct WorldTextPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: Option<(wgpu::Buffer, usize)>,
    index_buffer: Option<(wgpu::Buffer, usize)>,
}

impl WorldTextPipeline {
    /// `format` is the HUD's (non-sRGB) view of the surface; the pass is
    /// single sample against a `Depth32Float` depth view.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("World Text Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world_text_shader.wgsl").into()),
        });
        // The same entries as egui-wgpu's texture layout. wgpu pools layouts
        // by their entries, so egui's own bind groups fit this pipeline.
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("World Text Texture Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("World Text Pipeline Layout"),
            bind_group_layouts: &[&texture_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("World Text Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_world_text"),
                buffers: &[GlyphVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_world_text"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // egui's blend: premultiplied colour.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
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
            vertex_buffer: None,
            index_buffer: None,
        }
    }

    /// Record `mesh` onto `target`, tested against `depth`, with egui's
    /// textures from `egui_renderer`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        mesh: &GlyphMesh,
        egui_renderer: &egui_wgpu::Renderer,
    ) {
        if mesh.is_empty() {
            return;
        }
        let vertices = upload(
            device,
            queue,
            &mut self.vertex_buffer,
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = upload(
            device,
            queue,
            &mut self.index_buffer,
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("World Text"),
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
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        for (id, run) in &mesh.draws {
            let Some(texture) = egui_renderer.texture(id) else {
                continue;
            };
            pass.set_bind_group(0, &texture.bind_group, &[]);
            pass.draw_indexed(run.clone(), 0, 0..1);
        }
    }
}

/// Write `bytes` to the buffer in `slot`, growing it to the next power of
/// two when it's too small.
fn upload<'a>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &'a mut Option<(wgpu::Buffer, usize)>,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> &'a wgpu::Buffer {
    if slot.as_ref().is_none_or(|(_, capacity)| *capacity < bytes.len()) {
        let capacity = bytes.len().next_power_of_two();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("World Text Buffer"),
            size: capacity as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *slot = Some((buffer, capacity));
    }
    let (buffer, _) = slot.as_ref().expect("allocated above");
    queue.write_buffer(buffer, 0, bytes);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::world_text::TextPlane;

    /// A camera at the origin looking down -z: 90° vertical fov, square
    /// 200-point screen, near 1, far 100. Column-major.
    fn view() -> TextView {
        let (n, f) = (1.0f32, 100.0f32);
        TextView {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, f / (n - f), -1.0],
                [0.0, 0.0, n * f / (n - f), 0.0],
            ],
            eye: [0.0; 3],
            screen: [200.0, 200.0],
            px_per_point: 1.0,
        }
    }

    fn text(at: [f32; 3], size: f32, plane: Option<TextPlane>) -> WorldText {
        WorldText {
            at,
            text: "Ada's Planks".into(),
            size,
            color: [1.0, 0.9, 0.8, 1.0],
            align: 1,
            valign: 1,
            stroke: None,
            font: None,
            spacing: 0.0,
            shadow: None,
            plane,
        }
    }

    fn facing_camera() -> Option<TextPlane> {
        Some(TextPlane { normal: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] })
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |_| {});
        ctx
    }

    fn build(texts: &[WorldText], v: &TextView) -> GlyphMesh {
        let ctx = context();
        let mut laid = Vec::new();
        let _ = ctx.run(Default::default(), |ctx| {
            laid = lay_out(ctx, texts, v, &HashSet::new(), &mut HashSet::new());
        });
        tessellate(&ctx, laid, v)
    }

    fn bounds(mesh: &GlyphMesh) -> ([f32; 2], [f32; 2]) {
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        for v in &mesh.vertices {
            for i in 0..2 {
                let n = v.position[i] / v.position[3];
                lo[i] = lo[i].min(n);
                hi[i] = hi[i].max(n);
            }
        }
        (lo, hi)
    }

    #[test]
    fn a_billboard_is_centred_on_its_point_at_its_depth() {
        let v = view();
        let mesh = build(&[text([0.0, 0.0, -10.0], 14.0, None)], &v);
        assert!(!mesh.is_empty());
        let (lo, hi) = bounds(&mesh);
        // Centred within a pixel (0.01 NDC on a 200 px screen).
        assert!((lo[0] + hi[0]).abs() < 0.02, "{lo:?} {hi:?}");
        assert!((lo[1] + hi[1]).abs() < 0.02, "{lo:?} {hi:?}");
        // About 14 points tall: 0.14 NDC.
        assert!((hi[1] - lo[1] - 0.14).abs() < 0.05, "{lo:?} {hi:?}");
        let c = project(&v.view_proj, toward([0.0, 0.0, -10.0], v.eye));
        for g in &mesh.vertices {
            assert_eq!(g.position[3], 1.0);
            assert_eq!(g.position[2], c[2] / c[3]);
        }
        assert_eq!(mesh.draws.len(), 1);
        assert_eq!(mesh.draws[0].0, egui::TextureId::default());
        assert_eq!(mesh.draws[0].1, 0..mesh.indices.len() as u32);
    }

    #[test]
    fn behind_the_camera_or_past_the_far_plane_draws_nothing() {
        let v = view();
        assert!(build(&[text([0.0, 0.0, 10.0], 14.0, None)], &v).is_empty());
        assert!(build(&[text([0.0, 0.0, -500.0], 14.0, None)], &v).is_empty());
        assert!(build(&[text([0.0, 0.0, 10.0], 0.5, facing_camera())], &v).is_empty());
    }

    #[test]
    fn text_on_a_plane_is_as_big_as_asked_in_world_units() {
        // An em of 1 m at 4 m: 25 px, about a quarter of the half-screen.
        // (At a few pixels the glyphs' whole-pixel padding shows.)
        let v = view();
        let mesh = build(&[text([0.0, 0.0, -4.0], 1.0, facing_camera())], &v);
        assert!(!mesh.is_empty());
        let (lo, hi) = bounds(&mesh);
        let tall = hi[1] - lo[1];
        assert!((0.2..0.6).contains(&tall), "{tall}");
        // Twice as far, half as big, give or take the glyphs' pixel of
        // empty padding and the em's rounding to whole pixels.
        let far = build(&[text([0.0, 0.0, -8.0], 1.0, facing_camera())], &v);
        let (lo2, hi2) = bounds(&far);
        let half = hi2[1] - lo2[1];
        assert!((half * 2.0 - tall).abs() < 0.15 * tall, "{tall} vs {half}");
        // Each corner keeps its own depth.
        assert!(mesh.vertices.iter().all(|g| g.position[3] > 0.0));
    }

    #[test]
    fn text_on_a_plane_is_unseen_from_behind() {
        let away = Some(TextPlane { normal: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0] });
        assert!(build(&[text([0.0, 0.0, -10.0], 1.0, away)], &view()).is_empty());
    }

    #[test]
    fn text_on_a_plane_reaching_behind_the_camera_is_dropped_whole() {
        // A long name on a wall running past the camera's side.
        let wall = Some(TextPlane { normal: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] });
        let mut t = text([-2.0, 0.0, -1.5], 1.0, wall);
        t.text = "A very long name painted along the wall".into();
        assert!(build(&[t], &view()).is_empty());
    }

    #[test]
    fn plane_lengths_are_world_units_too() {
        // A 0.1 m shadow on a 1 m em moves with the text's scale.
        let v = view();
        let mut t = text([0.0, 0.0, -3.0], 1.0, facing_camera());
        t.shadow = Some(([0.0, 0.0, 0.0, 1.0], 0.1, 0.1));
        let near = build(std::slice::from_ref(&t), &v);
        t.at = [0.0, 0.0, -6.0];
        let far = build(&[t], &v);
        // Same layout either way, so the shadow's offset is the same share
        // of the text's height.
        let first_x = |m: &GlyphMesh| {
            let (lo, hi) = bounds(m);
            (m.vertices[0].position[0] / m.vertices[0].position[3] - lo[0]) / (hi[1] - lo[1])
        };
        assert!((first_x(&near) - first_x(&far)).abs() < 0.05);
    }

    #[test]
    fn several_labels_share_one_draw() {
        let mesh = build(
            &[text([0.0, 0.0, -10.0], 14.0, None), text([1.0, 1.0, -12.0], 1.0, facing_camera())],
            &view(),
        );
        assert_eq!(mesh.draws.len(), 1);
        assert_eq!(mesh.draws[0].1.end as usize, mesh.indices.len());
    }

    #[test]
    fn vertex_layout_matches_the_shader() {
        assert_eq!(std::mem::size_of::<GlyphVertex>(), 28);
    }
}
