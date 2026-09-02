//! `flint terrain-edit` — Interactive terrain editor with procgen + sculpt + paint.
//!
//! Opens a window with a 3D terrain viewport, procedural height generation,
//! direct brush sculpting, and splat-map painting. Operates on `.terrain.toml`
//! spec files and exports heightmap/splat PNGs.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Args;
use flint_core::Transform;
use flint_render::{Camera, OrbitCameraController, RenderContext, RendererConfig, SceneRenderer};
use flint_terrain::spec::{
    BlendMode, ErosionKind, ErosionLayer, FlattenLayer, FlattenMode, HeightLayer, NoiseLayer,
    NoiseType, SeedMode, SplatRule, TerrainSpec,
};
use flint_terrain::{
    apply_height_brush, apply_splat_brush, ray_heightmap_intersect, BrushParams, HeightBrushMode,
    Heightmap, SplatBrushMode, Terrain, TerrainConfig,
};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use super::gen_preview_common::random_seed;

// ─── CLI args ───────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct TerrainEditArgs {
    /// Path to a .terrain.toml spec file (created if it doesn't exist)
    pub spec: String,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Initial window width
    #[arg(long, default_value = "1440")]
    pub width: u32,

    /// Initial window height
    #[arg(long, default_value = "900")]
    pub height: u32,

    /// Disable the ground grid
    #[arg(long)]
    pub no_grid: bool,
}

pub fn run(args: TerrainEditArgs) -> Result<()> {
    let spec_path = PathBuf::from(&args.spec);

    let mut spec = if spec_path.exists() {
        TerrainSpec::from_file(&spec_path).map_err(|e| {
            anyhow::anyhow!("failed to load spec from {}: {}", spec_path.display(), e)
        })?
    } else {
        let name = spec_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("terrain")
            .replace(".terrain", "");
        let spec = TerrainSpec::default_spec(&name);
        spec.save(&spec_path).map_err(|e| {
            anyhow::anyhow!("failed to create spec at {}: {}", spec_path.display(), e)
        })?;
        tracing::info!("Created new terrain spec: {}", spec_path.display());
        spec
    };

    if let Some(seed_val) = args.seed {
        spec.seed.mode = SeedMode::Fixed;
        spec.seed.value = seed_val;
    }

    // File watcher
    let reload_flag = Arc::new(Mutex::new(false));
    let _watcher = {
        let flag = Arc::clone(&reload_flag);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
        debouncer
            .watcher()
            .watch(spec_path.as_ref(), RecursiveMode::NonRecursive)?;
        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_) => {
                        if let Ok(mut f) = flag.lock() {
                            *f = true;
                        }
                    }
                    Err(e) => tracing::warn!("Watch error: {:?}", e),
                }
            }
        });
        Some(debouncer)
    };

    let current_seed = if spec.seed.mode == SeedMode::Random {
        random_seed()
    } else {
        spec.seed.value
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = TerrainEditApp {
        spec,
        spec_path,
        reload_flag,
        _watcher,

        // Edit mode
        edit_mode: EditMode::Generate,
        height_brush_mode: HeightBrushMode::Raise,
        splat_brush_mode: SplatBrushMode::Paint,
        brush_params: BrushParams::default(),
        active_paint_layer: 0,

        // Procgen state
        heightmap_data: Vec::new(),
        splat_data: Vec::new(),
        current_seed,
        dirty: true,
        last_change: Instant::now() - Duration::from_secs(1),
        debounce_ms: 200,
        gen_time_ms: 0.0,

        // Brush state
        painting: false,
        brush_uv: None,
        needs_geometry_upload: false,
        needs_splat_upload: false,

        // Window / GPU
        args,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),

        // Input
        orbit: OrbitCameraController::new(),
        last_frame_time: Instant::now(),
        cursor_pos: None,

        // egui
        egui_ctx: egui::Context::default(),
        egui_winit: None,
        egui_renderer: None,
        show_ui: true,

        // FPS
        frame_times: VecDeque::new(),
        fps: 0.0,
        last_fps_update: Instant::now(),

        // Save state
        watcher_paused_until: None,
        status_message: None,
    };

    event_loop.run_app(&mut app)?;
    Ok(())
}

// ─── Edit modes ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    Generate,
    Sculpt,
    Paint,
}

// ─── App state ──────────────────────────────────────────────────────────────

struct TerrainEditApp {
    spec: TerrainSpec,
    spec_path: PathBuf,
    reload_flag: Arc<Mutex<bool>>,
    #[allow(dead_code)]
    _watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>,

    // Edit mode
    edit_mode: EditMode,
    height_brush_mode: HeightBrushMode,
    splat_brush_mode: SplatBrushMode,
    brush_params: BrushParams,
    active_paint_layer: u8,

    // Procgen state
    heightmap_data: Vec<f32>,
    splat_data: Vec<u8>,
    current_seed: u64,
    dirty: bool,
    last_change: Instant,
    debounce_ms: u64,
    gen_time_ms: f64,

    // Brush interaction
    painting: bool,
    brush_uv: Option<(f32, f32)>,
    needs_geometry_upload: bool,
    needs_splat_upload: bool,

    // Window / GPU
    args: TerrainEditArgs,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input
    orbit: OrbitCameraController,
    last_frame_time: Instant,
    cursor_pos: Option<(f64, f64)>,

    // egui
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_ui: bool,

    // FPS
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_fps_update: Instant,

    // Save state
    watcher_paused_until: Option<Instant>,
    status_message: Option<(String, Instant)>,
}

// ─── Initialization ─────────────────────────────────────────────────────────

impl TerrainEditApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let title = format!("Flint Terrain Edit \u{2014} {}", self.spec.meta.name);

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&title)
                        .with_inner_size(PhysicalSize::new(self.args.width, self.args.height)),
                )
                .expect("Failed to create window"),
        );

        let context = pollster::block_on(RenderContext::new(window.clone()))
            .expect("Failed to create render context");

        self.camera.aspect = context.aspect_ratio();
        // Position camera above and looking at terrain center
        let half_w = self.spec.geometry.width * 0.5;
        let half_d = self.spec.geometry.depth * 0.5;
        self.camera.target = flint_core::Vec3::new(half_w, 0.0, half_d);
        self.camera.distance = self.spec.geometry.width * 0.8;
        self.camera.pitch = 0.7;
        self.camera.yaw = std::f32::consts::FRAC_PI_4;
        self.camera.update_orbit();

        let renderer = SceneRenderer::new(
            &context,
            RendererConfig {
                show_grid: !self.args.no_grid,
                ..Default::default()
            },
        );

        // egui
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&context.device, context.config.format, None, 1, false);
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        self.window = Some(window);
        self.render_context = Some(context);
        self.scene_renderer = Some(renderer);
    }

    // ─── Terrain generation ──────────────────────────────────────────────

    fn regenerate(&mut self) {
        let start = Instant::now();

        let seed = self.current_seed;
        self.heightmap_data = flint_terrain::generate_heightmap(&self.spec, seed);

        let geo = &self.spec.geometry;
        self.splat_data = flint_terrain::generate_splat_map(
            &self.heightmap_data,
            geo.resolution,
            geo.resolution, // splat same res as height
            &self.spec.splat_rules,
            geo.width,
            geo.depth,
            geo.height_scale,
            seed,
        );

        self.gen_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        self.upload_full_terrain();
        self.dirty = false;
    }

    fn upload_full_terrain(&mut self) {
        let geo = &self.spec.geometry;
        let hm = Heightmap::from_raw(self.heightmap_data.clone(), geo.resolution, geo.resolution);
        let config = TerrainConfig {
            heightmap_path: String::new(),
            width: geo.width,
            depth: geo.depth,
            height_scale: geo.height_scale,
            chunk_resolution: geo.chunk_resolution,
            texture_tile: self.spec.textures.texture_tile,
            splat_map_path: String::new(),
            layer_textures: self.spec.layer_paths(),
            metallic: self.spec.textures.metallic,
            roughness: self.spec.textures.roughness,
            grass: None,
        };
        let terrain = Terrain::generate(&hm, &config);
        let spec_dir = self
            .spec_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let texture_tile = self.spec.textures.texture_tile;
        let metallic = self.spec.textures.metallic;
        let roughness = self.spec.textures.roughness;
        let splat_data = self.splat_data.clone();
        let layer_paths = self.spec.layer_paths();
        let resolution = geo.resolution;

        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.load_terrain_from_data(
                &context.device,
                &context.queue,
                &terrain.chunks,
                &Transform::default(),
                texture_tile,
                metallic,
                roughness,
                &splat_data,
                resolution,
                &layer_paths,
                &spec_dir,
            );
        }
    }

    fn upload_geometry_only(&mut self) {
        let geo = &self.spec.geometry;
        let hm = Heightmap::from_raw(self.heightmap_data.clone(), geo.resolution, geo.resolution);
        let config = TerrainConfig {
            heightmap_path: String::new(),
            width: geo.width,
            depth: geo.depth,
            height_scale: geo.height_scale,
            chunk_resolution: geo.chunk_resolution,
            texture_tile: self.spec.textures.texture_tile,
            splat_map_path: String::new(),
            layer_textures: self.spec.layer_paths(),
            metallic: self.spec.textures.metallic,
            roughness: self.spec.textures.roughness,
            grass: None,
        };
        let terrain = Terrain::generate(&hm, &config);

        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.reload_terrain_geometry(
                &context.device,
                &terrain.chunks,
                &Transform::default(),
            );
        }
    }

    fn upload_splat_only(&mut self) {
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            if let Some(tc) = renderer.texture_cache() {
                let _ = tc.update_rgba(&context.queue, "terrain_splat", &self.splat_data);
            }
        }
    }

    // ─── Brush interaction ───────────────────────────────────────────────

    fn update_brush_hit(&mut self) {
        let Some((cx, cy)) = self.cursor_pos else {
            self.brush_uv = None;
            return;
        };

        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };

        let w = context.config.width as f32;
        let h = context.config.height as f32;
        let ndc_x = (cx as f32 / w) * 2.0 - 1.0;
        let ndc_y = 1.0 - (cy as f32 / h) * 2.0;

        // Unproject through camera
        let (origin, dir) = self.camera.unproject_ray(ndc_x, ndc_y);

        let geo = &self.spec.geometry;
        self.brush_uv = ray_heightmap_intersect(
            origin,
            dir,
            &self.heightmap_data,
            geo.resolution,
            geo.width,
            geo.depth,
            geo.height_scale,
        );
    }

    fn apply_brush(&mut self, dt: f32) {
        let Some((u, v)) = self.brush_uv else {
            return;
        };

        let geo = &self.spec.geometry;
        match self.edit_mode {
            EditMode::Sculpt => {
                apply_height_brush(
                    &mut self.heightmap_data,
                    geo.resolution,
                    u,
                    v,
                    self.height_brush_mode,
                    &self.brush_params,
                    dt,
                    self.current_seed,
                );
                self.needs_geometry_upload = true;
            }
            EditMode::Paint => {
                apply_splat_brush(
                    &mut self.splat_data,
                    geo.resolution,
                    u,
                    v,
                    self.active_paint_layer,
                    self.splat_brush_mode,
                    &self.brush_params,
                    dt,
                );
                self.needs_splat_upload = true;
            }
            EditMode::Generate => {}
        }
    }

    // ─── File watching ───────────────────────────────────────────────────

    fn check_file_reload(&mut self) {
        if let Some(until) = self.watcher_paused_until {
            if Instant::now() < until {
                return;
            }
            self.watcher_paused_until = None;
        }

        let needs_reload = self
            .reload_flag
            .lock()
            .map(|mut f| {
                let v = *f;
                *f = false;
                v
            })
            .unwrap_or(false);

        if needs_reload {
            match TerrainSpec::from_file(&self.spec_path) {
                Ok(new_spec) => {
                    self.spec = new_spec;
                    self.mark_dirty();
                }
                Err(e) => tracing::warn!("Failed to reload spec: {}", e),
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn should_regenerate(&self) -> bool {
        self.dirty && self.last_change.elapsed() > Duration::from_millis(self.debounce_ms)
    }

    // ─── Save / Export ───────────────────────────────────────────────────

    fn save_spec(&mut self) {
        self.spec.seed.mode = SeedMode::Fixed;
        self.spec.seed.value = self.current_seed;

        match self.spec.save(&self.spec_path) {
            Ok(()) => {
                self.watcher_paused_until = Some(Instant::now() + Duration::from_millis(1000));
                self.set_status(&format!("Saved {}", self.spec_path.display()));
            }
            Err(e) => self.set_status(&format!("Save failed: {e}")),
        }
    }

    fn save_spec_as(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Save Terrain Spec As")
            .add_filter("Terrain Spec", &["toml"])
            .set_file_name(
                self.spec_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("terrain.terrain.toml"),
            );
        let dialog = if let Some(parent) = self.spec_path.parent() {
            dialog.set_directory(parent)
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            self.spec.seed.mode = SeedMode::Fixed;
            self.spec.seed.value = self.current_seed;
            match self.spec.save(&path) {
                Ok(()) => {
                    self.spec_path = path.clone();
                    self.watcher_paused_until = Some(Instant::now() + Duration::from_millis(1000));
                    self.set_status(&format!("Saved to {}", path.display()));
                }
                Err(e) => self.set_status(&format!("Save failed: {e}")),
            }
        }
    }

    fn export_maps(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Export Heightmap PNG")
            .add_filter("PNG Image", &["png"])
            .set_file_name("heightmap.png");
        let dialog = if let Some(parent) = self.spec_path.parent() {
            dialog.set_directory(parent)
        } else {
            dialog
        };

        if let Some(hm_path) = dialog.save_file() {
            let geo = &self.spec.geometry;
            let hm =
                Heightmap::from_raw(self.heightmap_data.clone(), geo.resolution, geo.resolution);

            match hm.save_png(&hm_path) {
                Ok(()) => {
                    // Also save splat map next to heightmap
                    let splat_path = hm_path.with_file_name(
                        hm_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("heightmap")
                            .to_owned()
                            + "_splat.png",
                    );
                    let res = geo.resolution;
                    let img = image::RgbaImage::from_raw(res, res, self.splat_data.clone())
                        .expect("splat data size mismatch");
                    match img.save(&splat_path) {
                        Ok(()) => {
                            self.set_status(&format!(
                                "Exported {} + {}",
                                hm_path.display(),
                                splat_path.display()
                            ));
                        }
                        Err(e) => {
                            self.set_status(&format!("Heightmap OK, splat failed: {e}"));
                        }
                    }
                }
                Err(e) => self.set_status(&format!("Export failed: {e}")),
            }
        }
    }

    fn set_status(&mut self, msg: &str) {
        tracing::info!("{}", msg);
        self.status_message = Some((msg.to_string(), Instant::now()));
    }

    // ─── egui UI ─────────────────────────────────────────────────────────

    fn render_egui(&mut self, target_view: &wgpu::TextureView) {
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return,
        };
        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };
        let egui_winit = match &mut self.egui_winit {
            Some(ew) => ew,
            None => return,
        };

        // Snapshot state for closure
        let mut spec = self.spec.clone();
        let mut current_seed = self.current_seed;
        let edit_mode = self.edit_mode;
        let mut height_brush_mode = self.height_brush_mode;
        let mut splat_brush_mode = self.splat_brush_mode;
        let mut brush_params = self.brush_params;
        let mut active_paint_layer = self.active_paint_layer;
        let gen_time_ms = self.gen_time_ms;
        let fps = self.fps;
        let brush_uv = self.brush_uv;
        let status_msg = self.status_message.clone();
        let heightmap_res = spec.geometry.resolution;

        let mut dirty = false;
        let mut new_mode = edit_mode;
        let mut want_save = false;
        let mut want_save_as = false;
        let mut want_export = false;
        let mut want_randomize = false;

        let raw_input = egui_winit.take_egui_input(&window);
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // ─── Left panel ──────────────────────────────────────────
            egui::SidePanel::left("left_panel")
                .default_width(320.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading(&spec.meta.name);
                    ui.separator();

                    // Mode tabs
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(new_mode == EditMode::Generate, "Generate")
                            .clicked()
                        {
                            new_mode = EditMode::Generate;
                        }
                        if ui
                            .selectable_label(new_mode == EditMode::Sculpt, "Sculpt")
                            .clicked()
                        {
                            new_mode = EditMode::Sculpt;
                        }
                        if ui
                            .selectable_label(new_mode == EditMode::Paint, "Paint")
                            .clicked()
                        {
                            new_mode = EditMode::Paint;
                        }
                    });
                    ui.separator();

                    // Seed control
                    ui.horizontal(|ui| {
                        ui.label("Seed:");
                        let mut seed_i64 = current_seed as i64;
                        if ui
                            .add(egui::DragValue::new(&mut seed_i64).speed(1))
                            .changed()
                        {
                            current_seed = seed_i64.max(0) as u64;
                            dirty = true;
                        }
                        if ui.button("Random").clicked() {
                            want_randomize = true;
                        }
                    });
                    ui.separator();

                    // Bottom buttons
                    egui::TopBottomPanel::bottom("left_bottom").show_inside(ui, |ui| {
                        ui.label(format!("Gen: {:.1} ms | FPS: {:.0}", gen_time_ms, fps));
                        ui.label(format!("Heightmap: {}x{}", heightmap_res, heightmap_res));

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                want_save = true;
                            }
                            if ui.button("Save As").clicked() {
                                want_save_as = true;
                            }
                            if ui.button("Export").clicked() {
                                want_export = true;
                            }
                        });

                        if let Some((msg, when)) = &status_msg {
                            let age = when.elapsed().as_secs_f32();
                            if age < 4.0 {
                                let alpha = if age > 3.0 { 1.0 - (age - 3.0) } else { 1.0 };
                                ui.label(egui::RichText::new(msg).small().color(
                                    egui::Color32::from_rgba_unmultiplied(
                                        180,
                                        220,
                                        180,
                                        (alpha * 255.0) as u8,
                                    ),
                                ));
                            }
                        }
                    });

                    // Mode-specific content
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match new_mode {
                                EditMode::Generate => {
                                    // Geometry params
                                    egui::CollapsingHeader::new("Geometry")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            dirty |= drag_f32(
                                                ui,
                                                "Width",
                                                &mut spec.geometry.width,
                                                1.0,
                                                1.0..=2048.0,
                                            );
                                            dirty |= drag_f32(
                                                ui,
                                                "Depth",
                                                &mut spec.geometry.depth,
                                                1.0,
                                                1.0..=2048.0,
                                            );
                                            dirty |= drag_f32(
                                                ui,
                                                "Height Scale",
                                                &mut spec.geometry.height_scale,
                                                0.5,
                                                0.0..=500.0,
                                            );
                                            let mut res = spec.geometry.resolution as i64;
                                            if drag_i64(ui, "Resolution", &mut res, 1, 4..=2048) {
                                                spec.geometry.resolution = res as u32;
                                                dirty = true;
                                            }
                                            let mut chunk_res =
                                                spec.geometry.chunk_resolution as i64;
                                            if drag_i64(ui, "Chunk Res", &mut chunk_res, 1, 2..=256)
                                            {
                                                spec.geometry.chunk_resolution = chunk_res as u32;
                                                dirty = true;
                                            }
                                        });

                                    // Height layers
                                    egui::CollapsingHeader::new(format!(
                                        "Height Layers ({})",
                                        spec.height_layers.len()
                                    ))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut remove_idx = None;
                                        for (i, layer) in spec.height_layers.iter_mut().enumerate()
                                        {
                                            let label = match layer {
                                                HeightLayer::Noise(n) => {
                                                    format!("[{}] Noise ({:?})", i, n.noise_type)
                                                }
                                                HeightLayer::Flatten(f) => {
                                                    format!("[{}] Flatten ({:?})", i, f.mode)
                                                }
                                                HeightLayer::Erosion(e) => {
                                                    format!("[{}] Erosion ({:?})", i, e.kind)
                                                }
                                            };
                                            egui::CollapsingHeader::new(&label)
                                                .id_salt(format!("hlayer_{i}"))
                                                .default_open(i == 0)
                                                .show(ui, |ui| {
                                                    dirty |= render_height_layer_ui(ui, layer, i);
                                                    if ui.small_button("Remove").clicked() {
                                                        remove_idx = Some(i);
                                                    }
                                                });
                                        }
                                        if let Some(idx) = remove_idx {
                                            spec.height_layers.remove(idx);
                                            dirty = true;
                                        }

                                        ui.horizontal(|ui| {
                                            if ui.button("+ Noise").clicked() {
                                                spec.height_layers.push(HeightLayer::Noise(
                                                    NoiseLayer {
                                                        noise_type: NoiseType::Perlin,
                                                        octaves: 4,
                                                        frequency: 2.0,
                                                        lacunarity: 2.0,
                                                        persistence: 0.5,
                                                        amplitude: 1.0,
                                                        blend: BlendMode::Add,
                                                    },
                                                ));
                                                dirty = true;
                                            }
                                            if ui.button("+ Flatten").clicked() {
                                                spec.height_layers.push(HeightLayer::Flatten(
                                                    FlattenLayer {
                                                        height: 0.1,
                                                        falloff: 0.05,
                                                        mode: FlattenMode::Below,
                                                    },
                                                ));
                                                dirty = true;
                                            }
                                            if ui.button("+ Erosion").clicked() {
                                                spec.height_layers.push(HeightLayer::Erosion(
                                                    ErosionLayer {
                                                        kind: ErosionKind::Thermal,
                                                        iterations: 50,
                                                        talus_angle: 0.6,
                                                        strength: 0.3,
                                                    },
                                                ));
                                                dirty = true;
                                            }
                                        });
                                    });

                                    // Splat rules
                                    egui::CollapsingHeader::new(format!(
                                        "Splat Rules ({})",
                                        spec.splat_rules.len()
                                    ))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut remove_idx = None;
                                        for (i, rule) in spec.splat_rules.iter_mut().enumerate() {
                                            egui::CollapsingHeader::new(format!(
                                                "[{}] Layer {}",
                                                i, rule.layer
                                            ))
                                            .id_salt(format!("srule_{i}"))
                                            .default_open(true)
                                            .show(
                                                ui,
                                                |ui| {
                                                    dirty |= render_splat_rule_ui(ui, rule, i);
                                                    if ui.small_button("Remove").clicked() {
                                                        remove_idx = Some(i);
                                                    }
                                                },
                                            );
                                        }
                                        if let Some(idx) = remove_idx {
                                            spec.splat_rules.remove(idx);
                                            dirty = true;
                                        }
                                        if ui.button("+ Splat Rule").clicked() {
                                            spec.splat_rules.push(SplatRule {
                                                layer: 0,
                                                height_min: 0.0,
                                                height_max: 1.0,
                                                slope_max: 1.0,
                                                noise_amount: 0.0,
                                                noise_scale: 4.0,
                                            });
                                            dirty = true;
                                        }
                                    });
                                }

                                EditMode::Sculpt => {
                                    ui.label("Height Brush");
                                    ui.separator();

                                    let modes = [
                                        (HeightBrushMode::Raise, "Raise"),
                                        (HeightBrushMode::Lower, "Lower"),
                                        (HeightBrushMode::Smooth, "Smooth"),
                                        (HeightBrushMode::Flatten, "Flatten"),
                                        (HeightBrushMode::Noise, "Noise"),
                                    ];
                                    ui.horizontal(|ui| {
                                        for (mode, label) in &modes {
                                            if ui
                                                .selectable_label(
                                                    height_brush_mode == *mode,
                                                    *label,
                                                )
                                                .clicked()
                                            {
                                                height_brush_mode = *mode;
                                            }
                                        }
                                    });

                                    ui.separator();
                                    render_brush_params_ui(ui, &mut brush_params);

                                    if let Some((u, v)) = brush_uv {
                                        ui.separator();
                                        ui.label(format!("UV: ({:.3}, {:.3})", u, v));
                                    }
                                }

                                EditMode::Paint => {
                                    ui.label("Splat Paint");
                                    ui.separator();

                                    ui.horizontal(|ui| {
                                        if ui
                                            .selectable_label(
                                                splat_brush_mode == SplatBrushMode::Paint,
                                                "Paint",
                                            )
                                            .clicked()
                                        {
                                            splat_brush_mode = SplatBrushMode::Paint;
                                        }
                                        if ui
                                            .selectable_label(
                                                splat_brush_mode == SplatBrushMode::Erase,
                                                "Erase",
                                            )
                                            .clicked()
                                        {
                                            splat_brush_mode = SplatBrushMode::Erase;
                                        }
                                    });

                                    ui.separator();
                                    ui.label("Active Layer:");
                                    ui.horizontal(|ui| {
                                        for i in 0..4u8 {
                                            let label = format!("{}", i);
                                            if ui
                                                .selectable_label(active_paint_layer == i, &label)
                                                .clicked()
                                            {
                                                active_paint_layer = i;
                                            }
                                        }
                                    });

                                    ui.separator();
                                    render_brush_params_ui(ui, &mut brush_params);

                                    if let Some((u, v)) = brush_uv {
                                        ui.separator();
                                        ui.label(format!("UV: ({:.3}, {:.3})", u, v));
                                    }
                                }
                            }
                        });
                });

            // ─── Right panel ──────────────────────────────────────────
            egui::SidePanel::right("right_panel")
                .default_width(280.0)
                .resizable(true)
                .show(ctx, |ui| {
                    // Texture layers
                    egui::CollapsingHeader::new("Texture Layers")
                        .default_open(true)
                        .show(ui, |ui| {
                            dirty |= render_texture_layer_ui(
                                ui,
                                "Layer 0 (R)",
                                &mut spec.textures.layers.layer0,
                                0,
                            );
                            dirty |= render_texture_layer_ui(
                                ui,
                                "Layer 1 (G)",
                                &mut spec.textures.layers.layer1,
                                1,
                            );
                            dirty |= render_texture_layer_ui(
                                ui,
                                "Layer 2 (B)",
                                &mut spec.textures.layers.layer2,
                                2,
                            );
                            dirty |= render_texture_layer_ui(
                                ui,
                                "Layer 3 (A)",
                                &mut spec.textures.layers.layer3,
                                3,
                            );
                        });

                    // PBR params
                    egui::CollapsingHeader::new("PBR")
                        .default_open(true)
                        .show(ui, |ui| {
                            dirty |= drag_f32(
                                ui,
                                "Texture Tile",
                                &mut spec.textures.texture_tile,
                                0.5,
                                0.1..=128.0,
                            );
                            dirty |= drag_f32(
                                ui,
                                "Metallic",
                                &mut spec.textures.metallic,
                                0.01,
                                0.0..=1.0,
                            );
                            dirty |= drag_f32(
                                ui,
                                "Roughness",
                                &mut spec.textures.roughness,
                                0.01,
                                0.0..=1.0,
                            );
                        });
                });

            // FPS overlay
            egui::Area::new(egui::Id::new("stats_overlay"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-290.0, 10.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(format!("FPS: {:.0}", fps));
                        let mode_label = match new_mode {
                            EditMode::Generate => "Generate",
                            EditMode::Sculpt => "Sculpt",
                            EditMode::Paint => "Paint",
                        };
                        ui.label(format!("Mode: {}", mode_label));
                    });
                });
        });

        // Tessellate and render egui
        egui_winit.handle_platform_output(&window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui Encoder"),
            });

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
        }

        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
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

            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
        }

        context.queue.submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);

        // Apply deferred mutations
        if dirty {
            self.spec = spec;
            self.current_seed = current_seed;
            self.mark_dirty();
        } else if current_seed != self.current_seed {
            self.current_seed = current_seed;
            self.mark_dirty();
        }
        self.edit_mode = new_mode;
        self.height_brush_mode = height_brush_mode;
        self.splat_brush_mode = splat_brush_mode;
        self.brush_params = brush_params;
        self.active_paint_layer = active_paint_layer;

        if want_randomize {
            self.current_seed = random_seed();
            self.mark_dirty();
        }
        if want_save {
            self.save_spec();
        }
        if want_save_as {
            self.save_spec_as();
        }
        if want_export {
            self.export_maps();
        }
    }
}

// ─── ApplicationHandler ─────────────────────────────────────────────────────

impl ApplicationHandler for TerrainEditApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.initialize(event_loop);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Forward to egui first
        if let Some(egui_winit) = &mut self.egui_winit {
            if let Some(window) = &self.window {
                let response = egui_winit.on_window_event(window, &event);
                if response.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(physical_size) => {
                if let Some(ctx) = &mut self.render_context {
                    ctx.resize(physical_size);
                    self.camera.aspect = ctx.aspect_ratio();
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.resize_postprocess(
                            &ctx.device,
                            physical_size.width,
                            physical_size.height,
                        );
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                self.orbit.handle_key_event(&event);

                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                        PhysicalKey::Code(KeyCode::Tab) => self.show_ui = !self.show_ui,
                        PhysicalKey::Code(KeyCode::Digit1) => {
                            self.edit_mode = EditMode::Generate;
                        }
                        PhysicalKey::Code(KeyCode::Digit2) => {
                            self.edit_mode = EditMode::Sculpt;
                        }
                        PhysicalKey::Code(KeyCode::Digit3) => {
                            self.edit_mode = EditMode::Paint;
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            self.current_seed = random_seed();
                            self.mark_dirty();
                        }
                        PhysicalKey::Code(KeyCode::KeyS) if !event.repeat => {
                            let mods = self.egui_ctx.input(|i| i.modifiers);
                            if mods.command && mods.shift {
                                self.save_spec_as();
                            } else if mods.command {
                                self.save_spec();
                            }
                        }
                        PhysicalKey::Code(KeyCode::KeyE) if !event.repeat => {
                            if self.egui_ctx.input(|i| i.modifiers.command) {
                                self.export_maps();
                            }
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            let half_w = self.spec.geometry.width * 0.5;
                            let half_d = self.spec.geometry.depth * 0.5;
                            self.camera.target = flint_core::Vec3::new(half_w, 0.0, half_d);
                            self.camera.distance = self.spec.geometry.width * 0.8;
                            self.camera.pitch = 0.7;
                            self.camera.yaw = std::f32::consts::FRAC_PI_4;
                            self.camera.update_orbit();
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            self.brush_params.radius = (self.brush_params.radius * 1.2).min(0.5);
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) => {
                            self.brush_params.radius = (self.brush_params.radius / 1.2).max(0.005);
                        }
                        _ => {}
                    }
                }
            }

            ref ev @ WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Some((position.x, position.y));

                if self.edit_mode == EditMode::Sculpt || self.edit_mode == EditMode::Paint {
                    self.update_brush_hit();
                }

                // Pass to orbit for right-drag
                self.orbit
                    .handle_event(ev, &mut self.camera, self.egui_ctx.is_pointer_over_area());
            }

            ref ev @ WindowEvent::MouseInput { state, button, .. } => {
                if matches!(
                    (self.edit_mode, button),
                    (EditMode::Sculpt | EditMode::Paint, MouseButton::Left)
                ) {
                    self.painting = state == ElementState::Pressed;
                }

                // Right-click orbit in sculpt/paint, all orbit in generate
                let pass_to_orbit = match self.edit_mode {
                    EditMode::Generate => true,
                    EditMode::Sculpt | EditMode::Paint => button == MouseButton::Right,
                };

                if pass_to_orbit {
                    self.orbit.handle_event(
                        ev,
                        &mut self.camera,
                        self.egui_ctx.is_pointer_over_area(),
                    );
                }
            }

            ref ev @ WindowEvent::MouseWheel { .. } => {
                self.orbit
                    .handle_event(ev, &mut self.camera, self.egui_ctx.is_pointer_over_area());
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self.last_frame_time.elapsed().as_secs_f32();
                self.last_frame_time = now;

                // Apply held keyboard orbit/zoom
                self.orbit.update(&mut self.camera, dt);

                // FPS tracking
                self.frame_times.push_back(now);
                while self
                    .frame_times
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
                {
                    self.frame_times.pop_front();
                }
                if now.duration_since(self.last_fps_update) > Duration::from_millis(250) {
                    self.fps = self.frame_times.len() as f32;
                    self.last_fps_update = now;
                }

                // File watcher
                self.check_file_reload();

                // Debounced regeneration
                if self.should_regenerate() {
                    self.regenerate();
                }

                // Brush painting (continuous while held)
                if self.painting && !self.egui_ctx.is_pointer_over_area() {
                    self.update_brush_hit();
                    self.apply_brush(dt);
                }

                // Upload changed data
                if self.needs_geometry_upload {
                    self.upload_geometry_only();
                    self.needs_geometry_upload = false;
                }
                if self.needs_splat_upload {
                    self.upload_splat_only();
                    self.needs_splat_upload = false;
                }

                let context = match &self.render_context {
                    Some(c) => c,
                    None => return,
                };

                let output = match context.surface.get_current_texture() {
                    Ok(o) => o,
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let size = context.size;
                        if let Some(ctx) = &mut self.render_context {
                            ctx.resize(size);
                        }
                        return;
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("Out of GPU memory");
                        event_loop.exit();
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("Surface error: {:?}", e);
                        return;
                    }
                };

                let view = output
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                // 3D render
                if let Some(renderer) = &mut self.scene_renderer {
                    let _ = renderer.render(context, &self.camera, &view);
                }

                // egui overlay
                if self.show_ui {
                    self.render_egui(&view);
                }

                output.present();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

// ─── UI helpers ─────────────────────────────────────────────────────────────

fn drag_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    speed: f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(format!("{label}:"));
        ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    (*value - before).abs() > f32::EPSILON
}

fn drag_f64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(format!("{label}:"));
        ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(4),
        );
    });
    (*value - before).abs() > f64::EPSILON
}

fn drag_i64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut i64,
    speed: i32,
    range: std::ops::RangeInclusive<i64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(format!("{label}:"));
        ui.add(egui::DragValue::new(value).speed(speed).range(range));
    });
    *value != before
}

fn render_height_layer_ui(ui: &mut egui::Ui, layer: &mut HeightLayer, _idx: usize) -> bool {
    let mut dirty = false;
    match layer {
        HeightLayer::Noise(n) => {
            // Noise type
            let types = [
                NoiseType::Perlin,
                NoiseType::Simplex,
                NoiseType::Value,
                NoiseType::Ridged,
            ];
            ui.horizontal(|ui| {
                ui.label("Type:");
                for t in &types {
                    if ui
                        .selectable_label(n.noise_type == *t, format!("{:?}", t))
                        .clicked()
                    {
                        n.noise_type = *t;
                        dirty = true;
                    }
                }
            });

            let mut octaves = n.octaves as i64;
            if drag_i64(ui, "Octaves", &mut octaves, 1, 1..=12) {
                n.octaves = octaves as u32;
                dirty = true;
            }
            dirty |= drag_f64(ui, "Frequency", &mut n.frequency, 0.1, 0.01..=100.0);
            dirty |= drag_f64(ui, "Lacunarity", &mut n.lacunarity, 0.05, 0.1..=10.0);
            dirty |= drag_f64(ui, "Persistence", &mut n.persistence, 0.01, 0.0..=1.0);
            dirty |= drag_f64(ui, "Amplitude", &mut n.amplitude, 0.05, 0.0..=10.0);

            // Blend mode
            let blends = [
                BlendMode::Add,
                BlendMode::Multiply,
                BlendMode::Max,
                BlendMode::Min,
                BlendMode::Overlay,
            ];
            ui.horizontal(|ui| {
                ui.label("Blend:");
                for b in &blends {
                    if ui
                        .selectable_label(n.blend == *b, format!("{:?}", b))
                        .clicked()
                    {
                        n.blend = *b;
                        dirty = true;
                    }
                }
            });
        }
        HeightLayer::Flatten(f) => {
            dirty |= drag_f64(ui, "Height", &mut f.height, 0.01, 0.0..=1.0);
            dirty |= drag_f64(ui, "Falloff", &mut f.falloff, 0.01, 0.0..=1.0);
            ui.horizontal(|ui| {
                ui.label("Mode:");
                if ui
                    .selectable_label(f.mode == FlattenMode::Below, "Below")
                    .clicked()
                {
                    f.mode = FlattenMode::Below;
                    dirty = true;
                }
                if ui
                    .selectable_label(f.mode == FlattenMode::Above, "Above")
                    .clicked()
                {
                    f.mode = FlattenMode::Above;
                    dirty = true;
                }
            });
        }
        HeightLayer::Erosion(e) => {
            ui.horizontal(|ui| {
                ui.label("Kind:");
                if ui
                    .selectable_label(e.kind == ErosionKind::Thermal, "Thermal")
                    .clicked()
                {
                    e.kind = ErosionKind::Thermal;
                    dirty = true;
                }
                if ui
                    .selectable_label(e.kind == ErosionKind::Hydraulic, "Hydraulic")
                    .clicked()
                {
                    e.kind = ErosionKind::Hydraulic;
                    dirty = true;
                }
            });
            let mut iters = e.iterations as i64;
            if drag_i64(ui, "Iterations", &mut iters, 1, 0..=1000) {
                e.iterations = iters as u32;
                dirty = true;
            }
            dirty |= drag_f64(ui, "Talus Angle", &mut e.talus_angle, 0.01, 0.0..=2.0);
            dirty |= drag_f64(ui, "Strength", &mut e.strength, 0.01, 0.0..=2.0);
        }
    }
    dirty
}

fn render_splat_rule_ui(ui: &mut egui::Ui, rule: &mut SplatRule, _idx: usize) -> bool {
    let mut dirty = false;
    let mut layer = rule.layer as i64;
    if drag_i64(ui, "Layer", &mut layer, 1, 0..=3) {
        rule.layer = layer as u8;
        dirty = true;
    }
    dirty |= drag_f32(ui, "Height Min", &mut rule.height_min, 0.01, 0.0..=1.0);
    dirty |= drag_f32(ui, "Height Max", &mut rule.height_max, 0.01, 0.0..=1.0);
    dirty |= drag_f32(ui, "Slope Max", &mut rule.slope_max, 0.01, 0.0..=1.0);
    dirty |= drag_f32(ui, "Noise Amt", &mut rule.noise_amount, 0.01, 0.0..=1.0);
    dirty |= drag_f32(ui, "Noise Scale", &mut rule.noise_scale, 0.1, 0.1..=100.0);
    dirty
}

fn render_brush_params_ui(ui: &mut egui::Ui, params: &mut BrushParams) {
    ui.horizontal(|ui| {
        ui.label("Radius:");
        ui.add(
            egui::DragValue::new(&mut params.radius)
                .speed(0.002)
                .range(0.005..=0.5)
                .max_decimals(3),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Strength:");
        ui.add(
            egui::DragValue::new(&mut params.strength)
                .speed(0.01)
                .range(0.01..=2.0)
                .max_decimals(2),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Falloff:");
        ui.add(
            egui::DragValue::new(&mut params.falloff)
                .speed(0.01)
                .range(0.0..=1.0)
                .max_decimals(2),
        );
    });
}

fn render_texture_layer_ui(
    ui: &mut egui::Ui,
    label: &str,
    layer: &mut Option<flint_terrain::spec::TextureLayer>,
    idx: usize,
) -> bool {
    let mut dirty = false;
    ui.horizontal(|ui| {
        ui.label(label);
        if let Some(l) = layer {
            let mut path = l.path.clone();
            if ui.text_edit_singleline(&mut path).changed() {
                l.path = path;
                dirty = true;
            }
            if ui.small_button("X").clicked() {
                *layer = None;
                dirty = true;
            }
        } else if ui.small_button("+").clicked() {
            *layer = Some(flint_terrain::spec::TextureLayer {
                path: String::new(),
                label: format!("Layer {idx}"),
            });
            dirty = true;
        }
    });
    dirty
}
