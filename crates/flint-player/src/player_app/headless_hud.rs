//! The HUD in a headless capture (#731): what scripts drew in `on_draw_ui`
//! and the UI documents they loaded, painted over a captured frame by the
//! player's own painter (`hud_render::render_draw_commands`), with the
//! project's fonts and sprites, so `flint render --scripts` shows the menu
//! the player shows.
//!
//! It paints the way the player's HUD pass does: an egui renderer built for
//! the target's format without its sRGB suffix, into a view of that format,
//! so colours blend in display space as egui's are authored (#428), on a
//! clock of 0 (egui's own animations never move a still).

use super::fonts::{font_definitions, script_families, text_measurer, ProjectFonts};
use super::hud_render::render_draw_commands;
use super::text_halo::TextHalos;
use flint_core::project::{AssetKind, AssetScope};
use flint_script::context::{DrawCommand, DrawItem, TextMeasurer};
use std::collections::{HashMap, HashSet};

/// egui's input for a still: the screen in points at a fixed scale, on a
/// clock of 0.
fn raw_input(points: [f32; 2], pixels_per_point: f32) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(points[0], points[1]),
        )),
        time: Some(0.0),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(pixels_per_point);
    input
}

/// The HUD painter of a headless capture: egui, its renderer and what it
/// draws with.
pub struct HeadlessHud {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
    textures: HashMap<String, egui::TextureHandle>,
    fonts: HashSet<String>,
    fonts_warned: HashSet<String>,
    sprites_missing: HashSet<String>,
    halos: TextHalos,
    scope: AssetScope,
}

impl HeadlessHud {
    /// A painter for targets of `format` (an sRGB format paints through its
    /// plain twin, which the target must list among its view formats), with
    /// the fonts and sprites `scope`'s scene sees.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        scope: AssetScope,
    ) -> Self {
        let format = format.remove_srgb_suffix();
        let ctx = egui::Context::default();
        let defs = font_definitions(&ProjectFonts::find(&scope.project, &scope.scene_dir));
        let fonts = script_families(&defs);
        ctx.set_fonts(defs);
        let renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                dithering: false,
                ..Default::default()
            },
        );
        let mut hud = Self {
            ctx,
            renderer,
            format,
            textures: HashMap::new(),
            fonts,
            fonts_warned: HashSet::new(),
            sprites_missing: HashSet::new(),
            halos: TextHalos::default(),
            scope,
        };
        // One empty pass builds the fonts, so the measurer is real from the
        // start, and its atlas goes up with it
        let output = hud.run([64.0, 64.0], 1.0, |_| {});
        hud.upload(device, queue, output.textures_delta);
        hud
    }

    /// The text measurer that lays text out as this painter draws it: for
    /// scripts' `measure_text` and the UI documents' layout.
    pub fn measurer(&self) -> TextMeasurer {
        text_measurer(self.ctx.clone(), None)
    }

    /// The format a view of the target must have to be painted into.
    pub fn view_format(&self) -> wgpu::TextureFormat {
        self.format
    }

    fn run(
        &mut self,
        points: [f32; 2],
        pixels_per_point: f32,
        ui: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        // A real frame: its texture updates are uploaded by the caller (#944)
        #[allow(clippy::disallowed_methods)]
        let output = self.ctx.run_ui(raw_input(points, pixels_per_point), ui);
        output
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut delta: egui::TexturesDelta,
    ) {
        for (id, images) in &delta.set {
            for image in images {
                self.renderer.update_texture(device, queue, *id, image);
            }
        }
        for id in &delta.free {
            self.renderer.free_texture(id);
        }
        // Applied above: egui 0.36 asserts when a delta drops unhandled
        delta.clear();
    }

    /// Load the sprites `items` name that aren't loaded, by the project's
    /// one search (`sprites/<name>`, then `<name>`), as the player does. A
    /// miss is warned about once.
    fn load_sprites(&mut self, items: &[DrawItem]) {
        for item in items {
            let DrawCommand::Sprite { name, .. } = &item.cmd else {
                continue;
            };
            if self.textures.contains_key(name) || self.sprites_missing.contains(name) {
                continue;
            }
            let found = self.scope.resolve(AssetKind::Sprites, name);
            let loaded = match found {
                Ok(path) => flint_render::open_image(&path)
                    .map_err(|e| format!("UI sprite '{name}': {}: {e}", path.display())),
                Err(missing) => Err(missing.to_string()),
            };
            match loaded {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    let handle = self
                        .ctx
                        .load_texture(name, image, egui::TextureOptions::LINEAR);
                    self.textures.insert(name.clone(), handle);
                }
                Err(problem) => {
                    tracing::warn!("{problem}");
                    self.sprites_missing.insert(name.clone());
                }
            }
        }
    }

    /// Paint `items` over `view` (of [`Self::view_format`], `size` pixels),
    /// at `pixels_per_point`, and submit. Nothing to paint is no pass.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: [u32; 2],
        pixels_per_point: f32,
        items: &[DrawItem],
    ) {
        if items.is_empty() {
            return;
        }
        self.load_sprites(items);
        self.halos.begin_frame();
        let points = [
            size[0] as f32 / pixels_per_point,
            size[1] as f32 / pixels_per_point,
        ];
        let (textures, fonts) = (&self.textures, &self.fonts);
        let (fonts_warned, halos) = (&mut self.fonts_warned, &mut self.halos);
        let ctx = self.ctx.clone();
        // A real frame: its texture updates are uploaded below (#944)
        #[allow(clippy::disallowed_methods)]
        let output = ctx.run_ui(raw_input(points, pixels_per_point), |ui| {
            let ctx = ui.ctx().clone();
            render_draw_commands(&ctx, items, textures, fonts, fonts_warned, halos);
        });
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: output.pixels_per_point,
        };
        let free = output.textures_delta.free.clone();
        let mut delta = output.textures_delta;
        for (id, images) in &delta.set {
            for image in images {
                self.renderer.update_texture(device, queue, *id, image);
            }
        }
        delta.set.clear();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Headless HUD"),
        });
        self.renderer
            .update_buffers(device, queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Headless HUD Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        queue.submit(Some(encoder.finish()));
        for id in &free {
            self.renderer.free_texture(id);
        }
        delta.clear();
    }
}
