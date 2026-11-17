//! Where the frame debugger's [`Window`] runs (its two tabs, the frame and
//! the profiler): a window (winit 0.30, wgpu, egui), or no window at all,
//! one frame of it drawn into a PNG (`--screenshot`) for an agent, a test
//! or the book.

use super::profile::ProfileStart;
use super::ui::Start;
use super::viewer::ViewerGpu;
use super::window::{Opened, Tabs};
use anyhow::{Context, Result};
use flint_render::{AcquireFrame, HeadlessContext, RenderContext};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn egui_renderer(device: &wgpu::Device, format: wgpu::TextureFormat) -> egui_wgpu::Renderer {
    let mut renderer = egui_wgpu::Renderer::new(
        device,
        format,
        egui_wgpu::RendererOptions {
            depth_stencil_format: None,
            msaa_samples: 1,
            dithering: false,
            ..Default::default()
        },
    );
    renderer
        .callback_resources
        .insert(ViewerGpu::new(device, format));
    renderer
}

/// Upload egui's textures, record its frame into `view`, submit, and free
/// what it let go.
fn paint(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    mut output: egui::FullOutput,
    view: &wgpu::TextureView,
    size: [u32; 2],
) {
    let ppp = output.pixels_per_point;
    let jobs = ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: ppp,
    };
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            renderer.update_texture(device, queue, *id, delta);
        }
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame-open"),
    });
    let mut buffers = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame-open egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.008,
                        g: 0.008,
                        b: 0.01,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    buffers.push(encoder.finish());
    queue.submit(buffers);
    for id in &output.textures_delta.free {
        renderer.free_texture(id);
    }
    // Applied above: egui 0.36 asserts when a delta drops unhandled (#944)
    output.textures_delta.clear();
}

// ── The window ──

/// What the window opens with.
pub struct Launch {
    pub opened: Opened,
    pub start: Start,
    pub profile: ProfileStart,
    pub on_profile: bool,
}

impl Launch {
    fn window(self, srgb_target: bool, max_side: u32) -> Tabs {
        Tabs::new(
            self.opened,
            &self.start,
            &self.profile,
            self.on_profile,
            srgb_target,
            max_side,
        )
    }
}

pub fn run_window(launch: Launch, size: [u32; 2]) -> Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut host = WindowHost {
        pending: Some(launch),
        size,
        gfx: None,
        next_repaint: None,
        error: None,
    };
    event_loop.run_app(&mut host).context("event loop")?;
    match host.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct Gfx {
    window: Arc<Window>,
    ctx: RenderContext,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    app: Tabs,
}

struct WindowHost {
    pending: Option<Launch>,
    size: [u32; 2],
    gfx: Option<Gfx>,
    next_repaint: Option<Instant>,
    error: Option<anyhow::Error>,
}

impl WindowHost {
    fn redraw(&mut self) {
        let Some(g) = &mut self.gfx else {
            return;
        };
        // The frame first: with none, egui doesn't run, so no texture
        // update is dropped unapplied (#945)
        let frame = match g.ctx.surface.acquire() {
            Ok(f) => f,
            Err(flint_render::SurfaceError::Lost | flint_render::SurfaceError::Outdated) => {
                g.ctx.resize(g.ctx.size);
                g.window.request_redraw();
                return;
            }
            Err(e) => {
                tracing::warn!("surface: {e}");
                return;
            }
        };
        let input = g.egui_winit.take_egui_input(&g.window);
        let app = &mut g.app;
        // A real frame: its texture updates are uploaded in `paint` (#944)
        #[allow(clippy::disallowed_methods)]
        let mut output = g.egui_ctx.run_ui(input, |ui| app.ui(ui));
        g.egui_winit
            .handle_platform_output(&g.window, std::mem::take(&mut output.platform_output));
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay);
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let size = [g.ctx.config.width, g.ctx.config.height];
        paint(
            &g.ctx.device,
            &g.ctx.queue,
            &mut g.renderer,
            &g.egui_ctx,
            output,
            &view,
            size,
        );
        g.window.pre_present_notify();
        g.ctx.queue.present(frame);
        match delay {
            Some(d) if d.is_zero() => g.window.request_redraw(),
            Some(d) if d < std::time::Duration::from_secs(3600) => {
                self.next_repaint = Some(Instant::now() + d);
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for WindowHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let Some(launch) = self.pending.take() else {
            return;
        };
        let title = launch.opened.title();
        let attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(self.size[0], self.size[1]));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(anyhow::anyhow!("creating the window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let ctx = match pollster::block_on(RenderContext::new(window.clone())) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(anyhow::anyhow!("starting the GPU: {e}"));
                event_loop.exit();
                return;
            }
        };
        let egui_ctx = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(ctx.device.limits().max_texture_dimension_2d as usize),
        );
        let format = ctx.config.format;
        let renderer = egui_renderer(&ctx.device, format);
        let max_side = ctx.device.limits().max_texture_dimension_2d;
        let app = launch.window(format.is_srgb(), max_side);
        self.gfx = Some(Gfx {
            window,
            ctx,
            egui_ctx,
            egui_winit,
            renderer,
            app,
        });
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(g) = &mut self.gfx else {
            return;
        };
        let response = g.egui_winit.on_window_event(&g.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                g.ctx.resize(size);
                g.window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {
                if response.repaint {
                    g.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.next_repaint {
            Some(t) if Instant::now() >= t => {
                self.next_repaint = None;
                if let Some(g) = &self.gfx {
                    g.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

// ── No window: one frame into a PNG ──

/// Draw the window at `size` pixels (`scale` pixels a point) into `out`,
/// once every image it shows is read, with the pointer at `hover`
/// (points) when given.
pub fn screenshot(
    launch: Launch,
    size: [u32; 2],
    scale: f32,
    hover: Option<[f32; 2]>,
    out: &Path,
) -> Result<()> {
    let hc = pollster::block_on(HeadlessContext::new(size[0], size[1]))
        .map_err(|e| anyhow::anyhow!("starting the GPU: {e}"))?;
    let format = hc.format;
    let mut renderer = egui_renderer(&hc.device, format);
    let ctx = egui::Context::default();
    let max_side = hc.device.limits().max_texture_dimension_2d;
    let mut app = launch.window(format.is_srgb(), max_side);
    let points = egui::vec2(size[0] as f32 / scale, size[1] as f32 / scale);
    let input = |frame: usize| {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, points)),
            time: Some(frame as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            max_texture_side: Some(hc.device.limits().max_texture_dimension_2d as usize),
            focused: true,
            events: hover
                .map(|[x, y]| vec![egui::Event::PointerMoved(egui::pos2(x, y))])
                .unwrap_or_default(),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        raw
    };
    // A replay asked for is drawn before anything is laid out (#1089)
    app.wait_for_replay();
    // A frame to lay out and ask for the images, a wait for them, then
    // frames until egui has settled (a window's first frame is its sizing)
    for frame in 0..6 {
        if frame == 1 {
            app.wait_for_images();
        }
        #[allow(clippy::disallowed_methods)] // a real frame: `paint` uploads its textures
        let output = ctx.run_ui(input(frame), |ui| app.ui(ui));
        paint(
            &hc.device,
            &hc.queue,
            &mut renderer,
            &ctx,
            output,
            &hc.color_view,
            size,
        );
        if app.loading() {
            app.wait_for_images();
        }
    }
    let pixels = pollster::block_on(hc.read_pixels())
        .map_err(|e| anyhow::anyhow!("reading the frame back: {e}"))?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    image::save_buffer(
        out,
        &pixels,
        size[0],
        size[1],
        image::ExtendedColorType::Rgba8,
    )
    .with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}
