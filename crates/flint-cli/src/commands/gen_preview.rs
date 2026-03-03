//! `flint gen-preview` — Interactive previewer for procedural generation specs.
//!
//! Opens a window with a 3D viewport (for mesh generators) or texture tabs
//! (for texture generators), plus an auto-generated parameter editor panel.
//! Parameters are derived from each generator's `param_schema()` JSON Schema.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use flint_core::components as comp;
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_procgen::{
    parse_param_schema, GeneratorOutput, GeneratorRegistry, MeshData, ParamFieldSpec,
    ParamFieldType, ProcGenSpec, SeedConfig, SeedMode,
};
use flint_render::{Camera, RenderContext, RendererConfig, SceneRenderer, Vertex as RenderVertex};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

// ─── CLI args ───────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct GenPreviewArgs {
    /// Path to a .procgen.toml spec file
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

pub fn run(args: GenPreviewArgs) -> Result<()> {
    let spec_path = PathBuf::from(&args.spec);
    let mut spec = ProcGenSpec::from_file(&spec_path)
        .with_context(|| format!("failed to load spec from {}", spec_path.display()))?;

    if let Some(seed_val) = args.seed {
        spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(seed_val),
            derive_from: None,
        };
    }

    // Build registry and resolve generator
    let mut registry = GeneratorRegistry::new();
    flint_procgen::register_built_in_generators(&mut registry);

    let generator = registry
        .get(&spec.generator)
        .with_context(|| format!("unknown generator '{}'", spec.generator))?;
    let schema = generator.param_schema();
    let field_specs = parse_param_schema(&schema);
    let output_kind = generator.output_kind();

    // Build initial param map from spec (or empty table)
    let initial_params = match &spec.params {
        toml::Value::Table(t) => t.clone(),
        _ => toml::map::Map::new(),
    };

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

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = GenPreviewApp {
        spec,
        spec_path,
        registry,
        field_specs,
        output_kind,
        reload_flag,
        _watcher,

        // Param editor
        params: initial_params,
        dirty: true, // generate on first frame
        last_change: Instant::now() - Duration::from_secs(1),
        debounce_ms: 200,

        // Seed
        current_seed: args.seed.unwrap_or(42),

        // Generation output
        last_mesh: None,
        last_images: Vec::new(),
        selected_image_tab: 0,
        gen_time_ms: 0.0,

        // Window / GPU
        args,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),

        // Input
        mouse_pressed: false,
        right_mouse_pressed: false,
        last_mouse_pos: None,
        last_frame_time: Instant::now(),

        // egui
        egui_ctx: egui::Context::default(),
        egui_winit: None,
        egui_renderer: None,
        egui_textures: Vec::new(),
        show_ui: true,

        // FPS
        frame_times: VecDeque::new(),
        fps: 0.0,
        last_fps_update: Instant::now(),

        // LOD
        lod_count: 0,
        selected_lod: 0,

        // Save state
        watcher_paused_until: None,
    };

    event_loop.run_app(&mut app)?;
    Ok(())
}

// ─── App state ──────────────────────────────────────────────────────────────

struct GenPreviewApp {
    spec: ProcGenSpec,
    spec_path: PathBuf,
    registry: GeneratorRegistry,
    field_specs: Vec<ParamFieldSpec>,
    #[allow(dead_code)]
    output_kind: flint_procgen::OutputKind,
    reload_flag: Arc<Mutex<bool>>,
    #[allow(dead_code)]
    _watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>,

    // Param editor
    params: toml::map::Map<String, toml::Value>,
    dirty: bool,
    last_change: Instant,
    debounce_ms: u64,

    // Seed
    current_seed: u64,

    // Generation output
    last_mesh: Option<MeshData>,
    last_images: Vec<flint_procgen::ImageData>,
    selected_image_tab: usize,
    gen_time_ms: f64,

    // Window / GPU
    args: GenPreviewArgs,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input
    mouse_pressed: bool,
    right_mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    last_frame_time: Instant,

    // egui
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    egui_textures: Vec<(String, egui::TextureHandle)>,
    show_ui: bool,

    // FPS
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_fps_update: Instant,

    // LOD
    lod_count: usize,
    selected_lod: usize,

    // Save state: temporarily ignore file watcher events after saving
    watcher_paused_until: Option<Instant>,
}

// ─── Initialization ─────────────────────────────────────────────────────────

impl GenPreviewApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let title = format!(
            "Flint Gen Preview \u{2014} {}",
            self.spec.meta.name
        );

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

        let renderer = SceneRenderer::new(
            &context,
            RendererConfig {
                show_grid: !self.args.no_grid,
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

    // ─── Generation ─────────────────────────────────────────────────────

    fn regenerate(&mut self) {
        // Update spec params and seed
        self.spec.params = toml::Value::Table(self.params.clone());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };

        let start = Instant::now();
        let result = self.registry.generate_from_spec(&self.spec);
        self.gen_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(output) => match output {
                GeneratorOutput::Mesh(mesh) => {
                    self.upload_mesh(&mesh, 0);
                    self.last_mesh = Some(mesh);
                    self.last_images.clear();
                    self.lod_count = 1;
                    self.selected_lod = 0;
                }
                GeneratorOutput::MeshWithLods(lods) => {
                    self.lod_count = lods.len();
                    if self.selected_lod >= self.lod_count {
                        self.selected_lod = 0;
                    }
                    if let Some(mesh) = lods.get(self.selected_lod) {
                        self.upload_mesh(mesh, self.selected_lod);
                        self.last_mesh = Some(mesh.clone());
                    }
                    self.last_images.clear();
                }
                GeneratorOutput::Image(img) => {
                    self.last_mesh = None;
                    self.last_images = vec![img];
                    self.update_egui_textures();
                }
                GeneratorOutput::ImageSet(images) => {
                    self.last_mesh = None;
                    self.last_images = images;
                    if self.selected_image_tab >= self.last_images.len() {
                        self.selected_image_tab = 0;
                    }
                    self.update_egui_textures();
                }
                GeneratorOutput::Sound(_) => {
                    tracing::warn!("Sound output not supported in gen-preview");
                }
            },
            Err(e) => {
                tracing::error!("Generation failed: {}", e);
            }
        }

        self.dirty = false;
    }

    /// Convert procgen MeshData to render vertices and upload to the GPU.
    fn upload_mesh(&mut self, mesh: &MeshData, _lod: usize) {
        if !mesh.submeshes.is_empty() {
            self.upload_submeshes(mesh);
            return;
        }

        // Single-material path (no submeshes)
        let base_color = mesh
            .materials
            .first()
            .map(|m| m.base_color)
            .unwrap_or([0.6, 0.6, 0.6, 1.0]);
        let metallic = mesh.materials.first().map(|m| m.metallic).unwrap_or(0.0);
        let roughness = mesh.materials.first().map(|m| m.roughness).unwrap_or(0.5);

        let render_vertices: Vec<RenderVertex> = mesh
            .vertices
            .iter()
            .map(|v| RenderVertex {
                position: v.position,
                normal: v.normal,
                color: base_color,
                uv: v.uv,
            })
            .collect();

        let material = flint_import::ImportedMaterial {
            name: "procgen_material".to_string(),
            base_color,
            metallic,
            roughness,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: false,
            alpha_mode: flint_import::AlphaMode::Opaque,
            alpha_cutoff: 0.5,
        };

        if let (Some(context), Some(renderer)) =
            (&self.render_context, &mut self.scene_renderer)
        {
            renderer.load_procedural_mesh(
                &context.device,
                "procgen_preview",
                &render_vertices,
                &mesh.indices,
                material,
            );
        }

        let world = build_mesh_world(1);
        if let (Some(context), Some(renderer)) =
            (&self.render_context, &mut self.scene_renderer)
        {
            renderer.update_from_world(&world, &context.device);
        }

        self.auto_fit_to_mesh(mesh);
    }

    /// Upload one procedural mesh per submesh, each with the correct material color.
    fn upload_submeshes(&mut self, mesh: &MeshData) {
        for (i, sub) in mesh.submeshes.iter().enumerate() {
            let mat = mesh
                .materials
                .get(sub.material_index)
                .cloned()
                .unwrap_or_default();

            // Collect the referenced indices and remap to a compact vertex set
            let start = sub.index_start as usize;
            let end = start + sub.index_count as usize;
            let sub_indices = &mesh.indices[start..end];

            // Find unique vertices and build a remapping
            let mut vertex_map: std::collections::HashMap<u32, u32> =
                std::collections::HashMap::new();
            let mut compact_verts: Vec<RenderVertex> = Vec::new();
            let mut compact_indices: Vec<u32> = Vec::new();

            for &orig_idx in sub_indices {
                let new_idx = *vertex_map.entry(orig_idx).or_insert_with(|| {
                    let idx = compact_verts.len() as u32;
                    let v = &mesh.vertices[orig_idx as usize];
                    compact_verts.push(RenderVertex {
                        position: v.position,
                        normal: v.normal,
                        color: mat.base_color,
                        uv: v.uv,
                    });
                    idx
                });
                compact_indices.push(new_idx);
            }

            let mesh_name = format!("procgen_preview_{i}");
            let material = flint_import::ImportedMaterial {
                name: format!("procgen_material_{i}"),
                base_color: mat.base_color,
                metallic: mat.metallic,
                roughness: mat.roughness,
                base_color_texture: None,
                normal_texture: None,
                metallic_roughness_texture: None,
                use_vertex_color: false,
                alpha_mode: flint_import::AlphaMode::Opaque,
                alpha_cutoff: 0.5,
            };

            if let (Some(context), Some(renderer)) =
                (&self.render_context, &mut self.scene_renderer)
            {
                renderer.load_procedural_mesh(
                    &context.device,
                    &mesh_name,
                    &compact_verts,
                    &compact_indices,
                    material,
                );
            }
        }

        let world = build_mesh_world(mesh.submeshes.len());
        if let (Some(context), Some(renderer)) =
            (&self.render_context, &mut self.scene_renderer)
        {
            renderer.update_from_world(&world, &context.device);
        }

        self.auto_fit_to_mesh(mesh);
    }

    // mesh_world is now build_mesh_world() free function below

    fn auto_fit_to_mesh(&mut self, mesh: &MeshData) {
        let bb = &mesh.bounding_box;
        let center = bb.center();
        let size = bb.size();
        let diag =
            (size.x * size.x + size.y * size.y + size.z * size.z).sqrt();

        self.camera.target = Vec3::new(center.x, center.y, center.z);
        self.camera.distance = (diag * 1.2).max(2.0);
        self.camera.yaw = std::f32::consts::FRAC_PI_4;
        self.camera.pitch = 0.5;
        self.camera.update_orbit();
    }

    /// Convert ImageData to egui texture handles for display.
    fn update_egui_textures(&mut self) {
        self.egui_textures.clear();
        for img in &self.last_images {
            let label = format!("{:?}", img.channel_semantics);
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.pixels,
            );
            let handle = self.egui_ctx.load_texture(
                &label,
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.egui_textures.push((label, handle));
        }
    }

    // ─── File watching ──────────────────────────────────────────────────

    fn check_file_reload(&mut self) {
        // Skip if watcher is paused (after save)
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
            match ProcGenSpec::from_file(&self.spec_path) {
                Ok(new_spec) => {
                    // Update params from disk
                    if let toml::Value::Table(t) = &new_spec.params {
                        self.params = t.clone();
                    }
                    self.spec = new_spec;
                    self.mark_dirty();
                }
                Err(e) => {
                    tracing::warn!("Failed to reload spec: {}", e);
                }
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn should_regenerate(&self) -> bool {
        self.dirty
            && self.last_change.elapsed() > Duration::from_millis(self.debounce_ms)
    }

    // ─── Save spec ──────────────────────────────────────────────────────

    fn save_spec(&mut self) {
        self.spec.params = toml::Value::Table(self.params.clone());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };

        match toml::to_string_pretty(&self.spec) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&self.spec_path, &content) {
                    tracing::error!("Failed to save spec: {}", e);
                } else {
                    // Pause watcher to avoid reload loop
                    self.watcher_paused_until =
                        Some(Instant::now() + Duration::from_millis(1000));
                    tracing::info!("Spec saved to {}", self.spec_path.display());
                }
            }
            Err(e) => {
                tracing::error!("Failed to serialize spec: {}", e);
            }
        }
    }

    // ─── egui parameter panel ───────────────────────────────────────────

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

        // Snapshot state for the closure
        let mut params = self.params.clone();
        let field_specs = self.field_specs.clone();
        let mut current_seed = self.current_seed;
        let gen_time_ms = self.gen_time_ms;
        let last_mesh = self.last_mesh.as_ref();
        let lod_count = self.lod_count;
        let mut selected_lod = self.selected_lod;
        let fps = self.fps;
        let spec_name = self.spec.meta.name.clone();
        let generator_name = self.spec.generator.clone();
        let is_texture = !self.last_images.is_empty();
        let mut selected_tab = self.selected_image_tab;
        let egui_textures = &self.egui_textures;

        let mut dirty = false;
        let mut want_save = false;
        let mut want_reset = false;
        let mut want_randomize = false;

        let raw_input = egui_winit.take_egui_input(&window);
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // Left side panel — parameter editor
            egui::SidePanel::left("param_panel")
                .default_width(320.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading(&spec_name);
                    ui.label(format!("Generator: {}", generator_name));
                    ui.separator();

                    // Seed control
                    ui.horizontal(|ui| {
                        ui.label("Seed:");
                        let mut seed_i64 = current_seed as i64;
                        if ui.add(egui::DragValue::new(&mut seed_i64).speed(1)).changed() {
                            current_seed = seed_i64.max(0) as u64;
                            dirty = true;
                        }
                        if ui.button("Randomize").clicked() {
                            want_randomize = true;
                        }
                    });
                    ui.separator();

                    // Parameter groups
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let groups = group_fields(&field_specs);
                            for (group_name, fields) in &groups {
                                egui::CollapsingHeader::new(group_name)
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        for field in fields {
                                            if render_param_field(ui, field, &mut params) {
                                                dirty = true;
                                            }
                                        }
                                    });
                            }
                        });

                    ui.separator();

                    // LOD selector
                    if lod_count > 1 {
                        ui.horizontal(|ui| {
                            ui.label("LOD:");
                            for i in 0..lod_count {
                                if ui.selectable_label(selected_lod == i, format!("{}", i)).clicked() {
                                    selected_lod = i;
                                    dirty = true;
                                }
                            }
                        });
                        ui.separator();
                    }

                    // Stats
                    if let Some(mesh) = last_mesh {
                        ui.label(format!(
                            "Vertices: {}  Triangles: {}",
                            format_count(mesh.vertex_count() as u64),
                            format_count(mesh.triangle_count() as u64),
                        ));
                    }
                    ui.label(format!("Gen time: {:.1} ms", gen_time_ms));
                    ui.label(format!("FPS: {:.0}", fps));

                    ui.separator();

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui.button("Save Spec").clicked() {
                            want_save = true;
                        }
                        if ui.button("Reset").clicked() {
                            want_reset = true;
                        }
                    });
                });

            // Texture display in central panel (only for image output)
            if is_texture && !egui_textures.is_empty() {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Tab bar
                    ui.horizontal(|ui| {
                        for (i, (label, _)) in egui_textures.iter().enumerate() {
                            if ui.selectable_label(selected_tab == i, label).clicked() {
                                selected_tab = i;
                            }
                        }
                    });
                    ui.separator();

                    // Display selected texture
                    if let Some((_, handle)) = egui_textures.get(selected_tab) {
                        let available = ui.available_size();
                        let tex_size = handle.size_vec2();
                        let scale = (available.x / tex_size.x).min(available.y / tex_size.y).min(1.0);
                        let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);

                        ui.centered_and_justified(|ui| {
                            ui.image(egui::load::SizedTexture::new(
                                handle.id(),
                                display_size,
                            ));
                        });
                    }
                });
            }

            // Top-right FPS overlay (only when in 3D viewport mode)
            if !is_texture {
                egui::Area::new(egui::Id::new("stats_overlay"))
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 10.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(format!("FPS: {:.0}", fps));
                        });
                    });
            }
        });

        // Tessellate and render egui (before applying mutations — egui_winit borrow ends here)
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

        // Apply deferred mutations (after egui_winit borrow is released)
        if dirty {
            self.params = params;
            self.selected_lod = selected_lod;
            self.current_seed = current_seed;
            self.mark_dirty();
        }
        if selected_tab != self.selected_image_tab {
            self.selected_image_tab = selected_tab;
        }
        if want_randomize {
            self.current_seed = random_seed();
            self.mark_dirty();
        }
        if want_save {
            self.save_spec();
        }
        if want_reset {
            self.reload_from_disk();
        }
    }

    fn reload_from_disk(&mut self) {
        match ProcGenSpec::from_file(&self.spec_path) {
            Ok(new_spec) => {
                if let toml::Value::Table(t) = &new_spec.params {
                    self.params = t.clone();
                }
                self.spec = new_spec;
                self.mark_dirty();
            }
            Err(e) => {
                tracing::warn!("Failed to reload spec from disk: {}", e);
            }
        }
    }
}

// ─── ApplicationHandler ─────────────────────────────────────────────────────

impl ApplicationHandler for GenPreviewApp {
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
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(physical_size) => {
                if let Some(ctx) = &mut self.render_context {
                    ctx.resize(physical_size);
                    self.camera.aspect = ctx.aspect_ratio();
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.resize_postprocess(&ctx.device, physical_size.width, physical_size.height);
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::Tab) => {
                            self.show_ui = !self.show_ui;
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            self.current_seed = random_seed();
                            self.mark_dirty();
                        }
                        PhysicalKey::Code(KeyCode::KeyS) if !event.repeat => {
                            // Save spec (S key — only fires when egui doesn't have focus)
                            self.save_spec();
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            // Reset camera
                            if let Some(mesh) = &self.last_mesh {
                                let mesh_clone = mesh.clone();
                                self.auto_fit_to_mesh(&mesh_clone);
                            }
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if !self.egui_ctx.is_pointer_over_area() {
                    match button {
                        MouseButton::Left => {
                            self.mouse_pressed = state == ElementState::Pressed;
                        }
                        MouseButton::Right => {
                            self.right_mouse_pressed = state == ElementState::Pressed;
                        }
                        _ => {}
                    }
                    if state == ElementState::Released {
                        self.last_mouse_pos = None;
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if !self.egui_ctx.is_pointer_over_area() {
                    if let Some((lx, ly)) = self.last_mouse_pos {
                        let dx = position.x - lx;
                        let dy = position.y - ly;

                        if self.mouse_pressed {
                            // Orbit
                            self.camera.yaw -= dx as f32 * 0.005;
                            self.camera.pitch += dy as f32 * 0.005;
                            self.camera.pitch = self.camera.pitch.clamp(-1.4, 1.4);
                            self.camera.update_orbit();
                        } else if self.right_mouse_pressed {
                            // Pan
                            let right_x = self.camera.yaw.cos();
                            let right_z = -self.camera.yaw.sin();
                            let pan_speed = self.camera.distance * 0.002;
                            self.camera.target.x -= dx as f32 * right_x * pan_speed;
                            self.camera.target.z -= dx as f32 * right_z * pan_speed;
                            let up_y = self.camera.pitch.cos();
                            self.camera.target.y += dy as f32 * up_y * pan_speed;
                            self.camera.update_orbit();
                        }
                    }
                    self.last_mouse_pos = Some((position.x, position.y));
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if !self.egui_ctx.is_pointer_over_area() {
                    let scroll = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.01,
                    };
                    self.camera.distance *= 1.0 - scroll * 0.1;
                    self.camera.distance = self.camera.distance.max(0.1);
                    self.camera.update_orbit();
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let _dt = self.last_frame_time.elapsed().as_secs_f64();
                self.last_frame_time = now;

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

                // 3D render (mesh mode)
                if self.last_mesh.is_some() {
                    if let Some(renderer) = &mut self.scene_renderer {
                        let _ = renderer.render(context, &self.camera, &view);
                    }
                } else {
                    // Clear to dark gray for texture-only mode
                    let context = self.render_context.as_ref().unwrap();
                    let mut encoder = context.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: Some("Clear Encoder"),
                        },
                    );
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Clear Pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.12,
                                        g: 0.12,
                                        b: 0.14,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                    }
                    context.queue.submit(std::iter::once(encoder.finish()));
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

// ─── ECS helper ─────────────────────────────────────────────────────────────

/// Build a minimal ECS world with one entity referencing the "procgen_preview" mesh.
fn build_mesh_world(submesh_count: usize) -> FlintWorld {
    let mut world = FlintWorld::new();

    let count = submesh_count.max(1);
    for i in 0..count {
        let asset_name = if count == 1 {
            "procgen_preview".to_string()
        } else {
            format!("procgen_preview_{i}")
        };
        let entity_name = if count == 1 {
            "procgen_preview".to_string()
        } else {
            format!("procgen_preview_{i}")
        };

        let eid = world.spawn(&entity_name).expect("spawn entity");

        let transform = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".to_string(),
                toml::Value::Array(vec![
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                ]),
            );
            t
        });
        let _ = world.set_component(eid, comp::TRANSFORM, transform);

        let model = toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert(
                "asset".to_string(),
                toml::Value::String(asset_name),
            );
            m
        });
        let _ = world.set_component(eid, comp::MODEL, model);
    }

    world
}

// ─── Parameter UI helpers ───────────────────────────────────────────────────

/// Group fields by underscore prefix (e.g. "trunk_height" -> "Trunk").
/// Fields without an underscore go into "General".
fn group_fields(fields: &[ParamFieldSpec]) -> Vec<(String, Vec<&ParamFieldSpec>)> {
    let mut groups: Vec<(String, Vec<&ParamFieldSpec>)> = Vec::new();

    for field in fields {
        let group_name = if let Some(pos) = field.name.find('_') {
            let prefix = &field.name[..pos];
            let mut chars = prefix.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect::<String>() + chars.as_str();
                    upper
                }
                None => "General".to_string(),
            }
        } else {
            "General".to_string()
        };

        if let Some(group) = groups.iter_mut().find(|(name, _)| name == &group_name) {
            group.1.push(field);
        } else {
            groups.push((group_name, vec![field]));
        }
    }

    // Move "General" to the front if it exists
    if let Some(pos) = groups.iter().position(|(name, _)| name == "General") {
        if pos > 0 {
            let general = groups.remove(pos);
            groups.insert(0, general);
        }
    }

    groups
}

/// Render a single parameter field widget. Returns true if the value changed.
fn render_param_field(
    ui: &mut egui::Ui,
    field: &ParamFieldSpec,
    params: &mut toml::map::Map<String, toml::Value>,
) -> bool {
    // Display name: strip the group prefix for cleaner labels
    let display_name = if let Some(pos) = field.name.find('_') {
        &field.name[pos + 1..]
    } else {
        &field.name
    };

    match &field.field_type {
        ParamFieldType::Float { min, max } => {
            let current = get_param_f64(params, &field.name, &field.default);
            let mut val = current;

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                let mut drag = egui::DragValue::new(&mut val).speed(0.01).max_decimals(4);
                if let (Some(lo), Some(hi)) = (min, max) {
                    drag = drag.range(*lo..=*hi);
                } else if let Some(lo) = min {
                    drag = drag.range(*lo..=f64::MAX);
                }
                ui.add(drag);
            });

            if (val - current).abs() > f64::EPSILON {
                params.insert(field.name.clone(), toml::Value::Float(val));
                return true;
            }
        }

        ParamFieldType::Integer { min, max } => {
            let current = get_param_i64(params, &field.name, &field.default);
            let mut val = current;

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                let mut drag = egui::DragValue::new(&mut val).speed(1);
                if let (Some(lo), Some(hi)) = (min, max) {
                    drag = drag.range(*lo..=*hi);
                } else if let Some(lo) = min {
                    drag = drag.range(*lo..=i64::MAX);
                }
                ui.add(drag);
            });

            if val != current {
                params.insert(field.name.clone(), toml::Value::Integer(val));
                return true;
            }
        }

        ParamFieldType::Bool => {
            let current = get_param_bool(params, &field.name, &field.default);
            let mut val = current;

            ui.checkbox(&mut val, display_name);

            if val != current {
                params.insert(field.name.clone(), toml::Value::Boolean(val));
                return true;
            }
        }

        ParamFieldType::HexColor => {
            let current_hex = get_param_string(params, &field.name, &field.default);
            let mut rgba = hex_to_rgba(&current_hex);

            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                changed = ui
                    .color_edit_button_srgba_unmultiplied(&mut rgba)
                    .changed();
                ui.monospace(&current_hex);
            });

            if changed {
                let new_hex = rgba_to_hex(&rgba);
                params.insert(field.name.clone(), toml::Value::String(new_hex));
                return true;
            }
        }

        ParamFieldType::Enum { values } => {
            let current = get_param_string(params, &field.name, &field.default);
            let mut selected = current.clone();

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                egui::ComboBox::from_id_salt(&field.name)
                    .selected_text(&selected)
                    .show_ui(ui, |ui| {
                        for v in values {
                            ui.selectable_value(&mut selected, v.clone(), v);
                        }
                    });
            });

            if selected != current {
                params.insert(field.name.clone(), toml::Value::String(selected));
                return true;
            }
        }

        ParamFieldType::String => {
            let current = get_param_string(params, &field.name, &field.default);
            let mut val = current.clone();

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                ui.text_edit_singleline(&mut val);
            });

            if val != current {
                params.insert(field.name.clone(), toml::Value::String(val));
                return true;
            }
        }

        ParamFieldType::StringArray { item_enum } => {
            let current_arr = get_param_string_array(params, &field.name, &field.default);
            let mut changed = false;

            if let Some(choices) = item_enum {
                // Multi-checkbox
                ui.label(format!("{}:", display_name));
                let mut selected: Vec<String> = current_arr.clone();
                for choice in choices {
                    let mut checked = selected.contains(choice);
                    if ui.checkbox(&mut checked, choice).changed() {
                        if checked && !selected.contains(choice) {
                            selected.push(choice.clone());
                        } else if !checked {
                            selected.retain(|s| s != choice);
                        }
                        changed = true;
                    }
                }
                if changed {
                    let arr = selected
                        .into_iter()
                        .map(toml::Value::String)
                        .collect::<Vec<_>>();
                    params.insert(field.name.clone(), toml::Value::Array(arr));
                }
            } else {
                // Display as read-only for now
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", display_name));
                    ui.monospace(format!("{:?}", current_arr));
                });
            }

            return changed;
        }
    }

    false
}

// ─── Param value extraction helpers ─────────────────────────────────────────

fn get_param_f64(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> f64 {
    if let Some(v) = params.get(key) {
        match v {
            toml::Value::Float(f) => return *f,
            toml::Value::Integer(i) => return *i as f64,
            _ => {}
        }
    }
    default
        .as_ref()
        .and_then(|d| d.as_f64())
        .unwrap_or(0.0)
}

fn get_param_i64(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> i64 {
    if let Some(v) = params.get(key) {
        match v {
            toml::Value::Integer(i) => return *i,
            toml::Value::Float(f) => return *f as i64,
            _ => {}
        }
    }
    default
        .as_ref()
        .and_then(|d| d.as_i64())
        .unwrap_or(0)
}

fn get_param_bool(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> bool {
    if let Some(toml::Value::Boolean(b)) = params.get(key) {
        return *b;
    }
    default
        .as_ref()
        .and_then(|d| d.as_bool())
        .unwrap_or(false)
}

fn get_param_string(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> String {
    if let Some(toml::Value::String(s)) = params.get(key) {
        return s.clone();
    }
    default
        .as_ref()
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string()
}

fn get_param_string_array(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> Vec<String> {
    if let Some(toml::Value::Array(arr)) = params.get(key) {
        return arr
            .iter()
            .filter_map(|v| {
                if let toml::Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
    }
    default
        .as_ref()
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ─── Color conversion helpers ───────────────────────────────────────────────

fn hex_to_rgba(hex: &str) -> [u8; 4] {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(hex.get(0..2).unwrap_or("80"), 16).unwrap_or(128);
    let g = u8::from_str_radix(hex.get(2..4).unwrap_or("80"), 16).unwrap_or(128);
    let b = u8::from_str_radix(hex.get(4..6).unwrap_or("80"), 16).unwrap_or(128);
    let a = if hex.len() > 6 {
        u8::from_str_radix(hex.get(6..8).unwrap_or("FF"), 16).unwrap_or(255)
    } else {
        255
    };
    [r, g, b, a]
}

fn rgba_to_hex(rgba: &[u8; 4]) -> String {
    if rgba[3] == 255 {
        format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2])
    } else {
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            rgba[0], rgba[1], rgba[2], rgba[3]
        )
    }
}

/// Generate a pseudo-random seed from system time.
fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    // Mix with a larger time component for better spread
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    (nanos ^ millis.wrapping_mul(6364136223846793005)) % 1_000_000
}

/// Human-friendly count with commas.
fn format_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
