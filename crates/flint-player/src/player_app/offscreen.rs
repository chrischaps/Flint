//! The player's frame with no window (#1191): a rendered playtest's
//! renderer draws the scene and the HUD into an offscreen colour texture
//! instead of a swapchain image, and nothing is presented.
//!
//! It is the window's frame, not a capture's: the same `PlayerApp`, ticked
//! the same way, then [`PlayerApp::render_offscreen`] in place of
//! `render`. The two share what the frame is drawn with
//! (`lay_frame_looks`: a look, its cross-fade, the scripts' post values),
//! `sync_renderer`'s `prepare` (it takes the offscreen device when there is
//! no window), the renderer's `encode_frame_to`, the subsystems'
//! `post_encode` and `render_hud`, whose egui input is the target's size
//! and no events. Screenshots (`--screenshot`, `--screenshot-every`, an
//! input script's `screenshot <file>`) are read back from the colour
//! texture after the frame's one submission.
//!
//! What a window's frame does that this one doesn't: present, the F3 and
//! F4 panels' sync (nothing opens them), GPU timing, frame captures and
//! frame grabs, the dynamic render scale, and `--exit-after`'s perf sums.

use super::subsystem::EncodeCtx;
use super::PlayerApp;
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::Mutex;

/// Offscreen targets made in this process and not lent now. A device is
/// made once and kept for the process (a playtest in a test binary runs
/// several), one a size: a run lends one and gives it back.
static IDLE: Mutex<Vec<&'static HeadlessContext>> = Mutex::new(Vec::new());

/// An offscreen target `width` x `height` pixels: an idle one of that size,
/// or a new device. `Err` when there is no GPU adapter.
pub fn lend(width: u32, height: u32) -> anyhow::Result<&'static HeadlessContext> {
    if width == 0 || height == 0 {
        anyhow::bail!("an offscreen frame of {width}x{height} has no pixels");
    }
    {
        let mut idle = IDLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = idle
            .iter()
            .position(|g| (g.width, g.height) == (width, height))
        {
            return Ok(idle.swap_remove(i));
        }
    }
    let gpu = pollster::block_on(HeadlessContext::new(width, height))
        .map_err(|e| anyhow::anyhow!("no GPU to draw on: {e}"))?;
    Ok(Box::leak(Box::new(gpu)))
}

/// Give a target from [`lend`] back once nothing draws into it.
pub fn give_back(gpu: &'static HeadlessContext) {
    IDLE.lock().unwrap_or_else(|e| e.into_inner()).push(gpu);
}

/// egui's input for an offscreen frame: the target's size in points at
/// `pixels_per_point`, and no events (nothing to click; an input script's
/// pointer reaches the game, not egui).
pub(super) fn raw_input(size: [u32; 2], pixels_per_point: f32) -> egui::RawInput {
    let ppp = pixels_per_point.max(0.01);
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size[0] as f32 / ppp, size[1] as f32 / ppp),
        )),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(ppp);
    input
}

/// What the HUD pass draws with: the window's device and target, or an
/// offscreen one's.
pub(super) struct HudTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// The target in pixels.
    pub size: [u32; 2],
    pub depth_view: &'a wgpu::TextureView,
    /// The target's format without its sRGB suffix, which the HUD draws
    /// through so egui blends in display space.
    pub hud_format: wgpu::TextureFormat,
    adapter: Adapter<'a>,
}

enum Adapter<'a> {
    Window(&'a wgpu::Adapter),
    Offscreen(&'a wgpu::AdapterInfo),
}

impl<'a> HudTarget<'a> {
    /// The window's target when there is one, else the offscreen one.
    pub fn of(
        window: Option<&'a flint_render::RenderContext>,
        offscreen: Option<&'static HeadlessContext>,
    ) -> Option<Self> {
        if let Some(c) = window {
            return Some(Self {
                device: &c.device,
                queue: &c.queue,
                size: [c.config.width, c.config.height],
                depth_view: &c.depth_view,
                hud_format: c.hud_format,
                adapter: Adapter::Window(&c.adapter),
            });
        }
        let g = offscreen?;
        Some(Self {
            device: &g.device,
            queue: &g.queue,
            size: [g.width, g.height],
            depth_view: &g.depth_view,
            hud_format: g.format.remove_srgb_suffix(),
            adapter: Adapter::Offscreen(&g.adapter_info),
        })
    }

    /// The adapter drawing, for the stats overlay's label.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        match self.adapter {
            Adapter::Window(a) => a.get_info(),
            Adapter::Offscreen(info) => info.clone(),
        }
    }
}

impl PlayerApp {
    /// A test's device or a rendered playtest's, when there is no window.
    pub(super) fn windowless_gpu(&self) -> Option<&'static HeadlessContext> {
        #[cfg(test)]
        if let Some(gpu) = self.test_gpu {
            return Some(gpu);
        }
        self.offscreen
    }

    /// Draw this player's frames into `gpu` (#1191), as `finish_initialize`
    /// sets a window's up: the renderer at the target's size, the camera's
    /// aspect, and the HUD's egui renderer. Before `enter_scene`, so the
    /// scene's models, styles and fonts load onto it.
    pub fn use_offscreen(&mut self, gpu: &'static HeadlessContext) {
        self.camera.aspect = gpu.width as f32 / gpu.height.max(1) as f32;
        let mut renderer = SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            gpu.format,
            gpu.width,
            gpu.height,
            RendererConfig {
                sample_count: self.launch.msaa_sample_count,
                ..Default::default()
            },
        );
        if let Some(knobs) = &self.launch.pass_knobs {
            renderer.set_pass_knobs(Some(knobs.clone()));
        }
        // egui sizes its atlas to the device, as `init_egui` has it
        let max_texture_side = gpu.device.limits().max_texture_dimension_2d as usize;
        self.hud
            .egui_ctx
            .input_mut(|i| i.max_texture_side = max_texture_side);
        self.hud.egui_renderer = Some(egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.format.remove_srgb_suffix(),
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                dithering: false,
                ..Default::default()
            },
        ));
        self.scene_renderer = Some(renderer);
        self.offscreen = Some(gpu);
    }

    /// The offscreen target this player draws into, to give back.
    pub fn offscreen(&self) -> Option<&'static HeadlessContext> {
        self.offscreen
    }

    /// Draw the frame `tick` just stepped into the offscreen target, scene
    /// then HUD, in one submission, and write the screenshots due on it.
    /// Nothing without [`use_offscreen`](Self::use_offscreen).
    pub fn render_offscreen(&mut self) {
        flint_core::scope!("render_offscreen");
        let Some(gpu) = self.offscreen else {
            return;
        };
        self.lay_frame_looks();
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Offscreen Frame Encoder"),
            });
        renderer.encode_frame_to(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            gpu.depth_target(),
            &self.camera,
            &gpu.color_view,
            Some(&self.core.world),
        );
        if !self.core.subsystems.is_empty() {
            let mut ctx = EncodeCtx {
                encoder: &mut encoder,
                device: &gpu.device,
                queue: &gpu.queue,
                output: &gpu.color_texture,
                depth_view: &gpu.depth_view,
                formats: (gpu.format, gpu.format.remove_srgb_suffix()),
                renderer: &mut *renderer,
                world: &self.core.world,
                camera: &self.camera,
                #[cfg(feature = "living")]
                living: &mut self.core.living_props,
            };
            self.core.subsystems.post_encode(&mut ctx);
        }
        // The HUD through the plain twin of the target, as the window's
        // `hud_view` (colours blend in display space)
        let hud_view = gpu.color_texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(gpu.format.remove_srgb_suffix()),
            ..Default::default()
        });
        self.render_hud(&hud_view, &mut encoder);
        gpu.queue.submit(std::iter::once(encoder.finish()));
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.frame_submitted();
        }

        self.capture.last_frame_time = Some(self.core.clock.total_time);
        let flagged = self.capture_path_due();
        let scripted = self.capture.take_scripted();
        if flagged.is_none() && scripted.is_empty() {
            return;
        }
        let pixels = pollster::block_on(gpu.read_pixels());
        if let Some(path) = flagged {
            match self.offscreen_shot(&pixels, gpu, &path) {
                Ok(shot) => {
                    self.capture.written += 1;
                    self.capture.shots.push(shot);
                }
                Err(reason) => {
                    tracing::warn!("screenshot: {reason}");
                    self.capture.note_failure(reason);
                }
            }
        }
        for path in scripted {
            match self.offscreen_shot(&pixels, gpu, &path) {
                Ok(shot) => self.capture.shots.push(shot),
                Err(reason) => {
                    tracing::warn!("screenshot: {reason}");
                    self.capture.scripted_failures.push(reason);
                }
            }
        }
    }

    fn offscreen_shot(
        &self,
        pixels: &Result<Vec<u8>, flint_render::RenderError>,
        gpu: &HeadlessContext,
        path: &std::path::Path,
    ) -> Result<super::ShotWritten, String> {
        let pixels = pixels
            .as_ref()
            .map_err(|e| format!("read-back of {} failed: {e}", path.display()))?;
        self.save_screenshot(pixels.clone(), gpu.width, gpu.height, path)
    }
}
