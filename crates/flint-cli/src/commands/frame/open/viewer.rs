//! The target viewer: one captured image in a pane, zoomed and panned,
//! with a readout of the texel under the cursor and an A/B wipe against
//! another image.
//!
//! The image is drawn by `viewer.wgsl` through an egui-wgpu paint callback:
//! its stored values go to the GPU once as an `Rgba32Float` texture, and the
//! exposure, channel, mode and depth range are uniforms, so a slider moves
//! the picture the frame it moves. The readout reads the CPU's copy, so the
//! numbers are the stored ones, never the displayed.

use super::super::display::{Channel, Display, Mode, Prepared};
use super::loader::Loaded;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use flint_render::TrustedShader;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const SHADER: &str = include_str!("viewer.wgsl");

/// What the viewer compares the image with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wipe {
    Off,
    /// The image of the pass before.
    Previous,
    /// The finished frame.
    Final,
}

/// How the viewer looks at its image, kept from pass to pass so stepping
/// keeps the framing and the exposure.
#[derive(Debug, Clone)]
pub struct ViewState {
    /// Points per texel; `None` fits the image to the pane.
    pub zoom: Option<f32>,
    /// The image centre's offset from the pane centre, points.
    pub pan: Vec2,
    /// Stops over the frame's exposure.
    pub ev: f32,
    /// `None`: what `flint frame image` would use.
    pub mode: Option<Mode>,
    pub channel: Channel,
    /// A depth range set by hand; `None` is the drawn depths'.
    pub depth: Option<(f32, f32)>,
    pub wipe: Wipe,
    /// Where the wipe's line is, 0 (left) to 1.
    pub split: f32,
    /// Lines between texels once they are this many pixels wide.
    pub grid_from: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: None,
            pan: Vec2::ZERO,
            ev: 0.0,
            mode: None,
            channel: Channel::Rgb,
            depth: None,
            wipe: Wipe::Off,
            split: 0.5,
            grid_from: 12.0,
        }
    }
}

impl ViewState {
    /// How `loaded` is shown under this state.
    pub fn display(&self, loaded: &Loaded) -> Display {
        let mut d = loaded.auto;
        if let Some(mode) = self.mode {
            d.mode = mode;
        }
        d.exposure = loaded.auto.exposure * 2f32.powf(self.ev);
        if let Some((lo, hi)) = self.depth {
            d.lo = lo;
            d.hi = hi;
        }
        d
    }

    /// The image's rect in the pane, points.
    pub fn placed(&self, pane: Rect, size: Vec2) -> Rect {
        let scale = self.scale(pane, size);
        Rect::from_center_size(pane.center() + self.pan, size * scale)
    }

    /// Points per texel.
    pub fn scale(&self, pane: Rect, size: Vec2) -> f32 {
        self.zoom.unwrap_or_else(|| fit(pane, size))
    }

    /// Zoom by `factor` keeping the point `at` (in the pane) where it is.
    pub fn zoom_at(&mut self, pane: Rect, size: Vec2, at: Pos2, factor: f32) {
        let old = self.scale(pane, size);
        let new = (old * factor).clamp(fit(pane, size).min(1.0) * 0.25, 256.0);
        let centre = pane.center() + self.pan;
        // The texel under `at` stays under it
        let off = (at - centre) * (new / old);
        self.pan = at - off - pane.center();
        self.zoom = Some(new);
    }

    pub fn fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }
}

/// Points per texel that fit `size` inside `pane` with a margin.
fn fit(pane: Rect, size: Vec2) -> f32 {
    let room = (pane.size() - Vec2::splat(16.0)).max(Vec2::splat(1.0));
    (room.x / size.x.max(1.0))
        .min(room.y / size.y.max(1.0))
        .max(1e-3)
}

/// The texel under `pos`, if it is on the image.
pub fn texel_at(placed: Rect, size: Vec2, pos: Pos2) -> Option<(u32, u32)> {
    if !placed.contains(pos) {
        return None;
    }
    let p = (pos - placed.min) / placed.size() * size;
    let (x, y) = (p.x.floor(), p.y.floor());
    (x >= 0.0 && y >= 0.0 && x < size.x && y < size.y).then_some((x as u32, y as u32))
}

/// What the cursor is over, for the readout.
pub struct Hover {
    pub texel: (u32, u32),
    /// As stored.
    pub values: Vec<f32>,
    /// As shown, 0..1.
    pub shown: [f32; 3],
    /// Over the wipe's other side.
    pub other: bool,
}

/// One side of the pane.
pub struct Side<'a> {
    pub key: usize,
    pub loaded: &'a Arc<Loaded>,
    pub display: Display,
}

/// Draw the viewer into `pane`: `a` on the right of the wipe (all of the
/// pane when there is no `b`), `b` on its left, both over the same rect.
pub fn show(
    ui: &mut egui::Ui,
    pane: Rect,
    state: &mut ViewState,
    a: Side,
    b: Option<Side>,
    srgb_target: bool,
) -> Option<Hover> {
    let response = ui.interact(pane, ui.id().with("frame-viewer"), Sense::click_and_drag());
    let pa = &a.loaded.prepared;
    let size = Vec2::new(pa.width as f32, pa.height as f32);

    // The wipe's line, and its handle
    let split_x = pane.left() + state.split * pane.width();
    let handle = Rect::from_center_size(
        Pos2::new(split_x, pane.center().y),
        Vec2::new(14.0, pane.height()),
    );
    let on_handle = b.is_some()
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| handle.contains(p));
    let drag_id = ui.id().with("wipe-drag");
    if response.drag_started() {
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp(drag_id, on_handle));
    }
    let dragging_split = b.is_some()
        && ui
            .ctx()
            .memory(|m| m.data.get_temp::<bool>(drag_id).unwrap_or(false));
    if response.dragged() {
        if dragging_split {
            if let Some(p) = response.interact_pointer_pos() {
                state.split = ((p.x - pane.left()) / pane.width()).clamp(0.0, 1.0);
            }
        } else {
            if state.zoom.is_none() {
                state.zoom = Some(state.scale(pane, size));
            }
            state.pan += response.drag_delta();
        }
    }
    if response.drag_stopped() {
        ui.ctx().memory_mut(|m| m.data.insert_temp(drag_id, false));
    }
    if response.double_clicked() {
        state.fit();
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(p) = response.hover_pos() {
                state.zoom_at(pane, size, p, (scroll / 120.0).exp2().clamp(0.25, 4.0));
            }
        }
    }
    if on_handle || dragging_split {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    let placed = state.placed(pane, size);
    let ppp = ui.ctx().pixels_per_point();
    let px_per_texel = placed.width() / size.x * ppp;
    let grid = ((px_per_texel - state.grid_from) / state.grid_from).clamp(0.0, 1.0);

    let painter = ui.painter_at(pane);
    let add = |side: &Side, clip: Rect, slot: usize| {
        let p = &side.loaded.prepared;
        let uniforms = uniforms(
            placed,
            p,
            &side.display,
            state.channel,
            grid,
            ppp,
            srgb_target,
        );
        let cb = ViewCallback {
            slot,
            key: side.key,
            loaded: Arc::clone(side.loaded),
            uniforms,
        };
        painter
            .with_clip_rect(clip)
            .add(egui_wgpu::Callback::new_paint_callback(pane, cb));
    };
    match &b {
        Some(b) => {
            add(&a, pane.intersect(Rect::everything_right_of(split_x)), 0);
            add(b, pane.intersect(Rect::everything_left_of(split_x)), 1);
            let ink = Color32::from_rgba_unmultiplied(240, 236, 226, 200);
            painter.line_segment(
                [
                    Pos2::new(split_x, pane.top()),
                    Pos2::new(split_x, pane.bottom()),
                ],
                Stroke::new(1.5, ink),
            );
            painter.circle_filled(Pos2::new(split_x, pane.center().y), 6.0, ink);
        }
        None => add(&a, pane, 0),
    }
    // The image's edge, a hairline
    painter.rect_stroke(
        placed,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(28)),
        egui::StrokeKind::Outside,
    );

    let pos = response.hover_pos()?;
    let other = b.is_some() && pos.x < split_x;
    let side = if other { b.as_ref()? } else { &a };
    let p = &side.loaded.prepared;
    let side_size = Vec2::new(p.width as f32, p.height as f32);
    let (x, y) = texel_at(placed, side_size, pos)?;
    let values = p.stored(x, y)?;
    let i = y as usize * p.width as usize + x as usize;
    let (picked, channels) = super::super::display::pick(p.texel(i), p.channels, state.channel);
    let shown = side.display.map(picked, channels);
    Some(Hover {
        texel: (x, y),
        values,
        shown,
        other,
    })
}

/// The shader's uniforms (`View` in `viewer.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewUniforms {
    origin: [f32; 2],
    scale: [f32; 2],
    size: [f32; 2],
    grid: f32,
    exposure: f32,
    lo: f32,
    hi: f32,
    max_v: f32,
    mode: u32,
    channel: u32,
    channels: u32,
    srgb_target: u32,
    _pad: u32,
}

fn uniforms(
    placed: Rect,
    p: &Prepared,
    d: &Display,
    channel: Channel,
    grid: f32,
    ppp: f32,
    srgb_target: bool,
) -> ViewUniforms {
    let size = [p.width as f32, p.height as f32];
    ViewUniforms {
        origin: [placed.min.x * ppp, placed.min.y * ppp],
        scale: [
            placed.width() * ppp / size[0].max(1.0),
            placed.height() * ppp / size[1].max(1.0),
        ],
        size,
        grid,
        exposure: d.exposure,
        lo: d.lo,
        hi: d.hi,
        max_v: d.max,
        mode: d.mode.code(),
        channel: if matches!(d.mode, Mode::Ids | Mode::Depth) {
            0
        } else {
            channel.code()
        },
        channels: p.channels as u32,
        srgb_target: srgb_target as u32,
        _pad: 0,
    }
}

/// One side's draw.
struct ViewCallback {
    slot: usize,
    key: usize,
    loaded: Arc<Loaded>,
    uniforms: ViewUniforms,
}

/// How many images the GPU keeps.
const KEEP_ON_GPU: usize = 10;

/// The viewer's GPU state, kept in egui-wgpu's callback resources.
pub struct ViewerGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// One uniform buffer a side.
    slots: [wgpu::Buffer; 2],
    /// Each image's texture, least recently drawn first.
    textures: VecDeque<(usize, wgpu::Texture)>,
    /// Each (side, image)'s bind group.
    binds: HashMap<(usize, usize), wgpu::BindGroup>,
    /// Images too big for the device (the window says so; see
    /// `App::max_side`).
    refused: std::collections::HashSet<usize>,
}

impl ViewerGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame-viewer"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame-viewer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<ViewUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frame-viewer"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame-viewer"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
        let slot = |n: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if n == 0 {
                    "frame-viewer a"
                } else {
                    "frame-viewer b"
                }),
                size: std::mem::size_of::<ViewUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            pipeline,
            layout,
            slots: [slot(0), slot(1)],
            textures: VecDeque::new(),
            binds: HashMap::new(),
            refused: Default::default(),
        }
    }

    fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: usize, p: &Prepared) {
        if let Some(at) = self.textures.iter().position(|(k, _)| *k == key) {
            let kept = self.textures.remove(at).expect("found above");
            self.textures.push_back(kept);
            return;
        }
        if self.refused.contains(&key) {
            return;
        }
        let max = device.limits().max_texture_dimension_2d;
        if p.width == 0 || p.height == 0 || p.width > max || p.height > max {
            self.refused.insert(key);
            return;
        }
        let size = wgpu::Extent3d {
            width: p.width,
            height: p.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame-viewer image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&p.rgba),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(p.width * 16),
                rows_per_image: Some(p.height),
            },
            size,
        );
        self.textures.push_back((key, texture));
        while self.textures.len() > KEEP_ON_GPU {
            if let Some((old, _)) = self.textures.pop_front() {
                self.binds.retain(|(_, k), _| *k != old);
            }
        }
    }

    fn bind(&mut self, device: &wgpu::Device, slot: usize, key: usize) -> bool {
        if self.binds.contains_key(&(slot, key)) {
            return true;
        }
        let Some((_, texture)) = self.textures.iter().find(|(k, _)| *k == key) else {
            return false;
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame-viewer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.slots[slot].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.binds.insert((slot, key), group);
        true
    }
}

impl egui_wgpu::CallbackTrait for ViewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<ViewerGpu>() {
            gpu.ensure(device, queue, self.key, &self.loaded.prepared);
            gpu.bind(device, self.slot, self.key);
            queue.write_buffer(&gpu.slots[self.slot], 0, bytemuck::bytes_of(&self.uniforms));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<ViewerGpu>() else {
            return;
        };
        let Some(group) = gpu.binds.get(&(self.slot, self.key)) else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewer_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("WGSL parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("WGSL validation");
    }

    #[test]
    fn the_uniforms_are_the_shaders_size() {
        // View in viewer.wgsl: 3 vec2, 5 f32, 5 u32
        assert_eq!(std::mem::size_of::<ViewUniforms>(), 64);
    }

    #[test]
    fn zooming_keeps_the_texel_under_the_cursor() {
        let pane = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let size = Vec2::new(64.0, 36.0);
        let mut s = ViewState::default();
        let at = Pos2::new(500.0, 250.0);
        let before = texel_at(s.placed(pane, size), size, at);
        s.zoom_at(pane, size, at, 4.0);
        let after = texel_at(s.placed(pane, size), size, at);
        assert_eq!(before, after);
        assert!(s.scale(pane, size) > 40.0);
        s.fit();
        assert_eq!(s.placed(pane, size).center(), pane.center());
    }

    #[test]
    fn a_texel_is_found_at_its_corner_and_not_past_the_edge() {
        let placed = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(64.0, 36.0));
        let size = Vec2::new(64.0, 36.0);
        assert_eq!(texel_at(placed, size, Pos2::new(10.0, 10.0)), Some((0, 0)));
        assert_eq!(
            texel_at(placed, size, Pos2::new(73.9, 45.9)),
            Some((63, 35))
        );
        assert_eq!(texel_at(placed, size, Pos2::new(74.5, 20.0)), None);
    }
}
