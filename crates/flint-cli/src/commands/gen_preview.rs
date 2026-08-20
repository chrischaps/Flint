//! `flint gen-preview` — Interactive previewer for procedural generation specs.
//!
//! Opens a window with a 3D viewport (for mesh generators) or texture tabs
//! (for texture generators), plus an auto-generated parameter editor panel.
//! Parameters are derived from each generator's `param_schema()` JSON Schema.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use flint_core::components as comp;
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_procgen::{
    export_glb, parse_param_schema, GeneratorOutput, GeneratorRegistry, MeshData, ParamFieldSpec,
    ProcGenSpec, SeedConfig, SeedMode,
};
use flint_render::{
    Camera, DebugMode, OrbitCameraController, RenderContext, RendererConfig, SceneRenderer,
    Vertex as RenderVertex,
};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
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

    /// Start with auto-orbit enabled
    #[arg(long)]
    pub auto_orbit: bool,
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

    let auto_orbit = args.auto_orbit;

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
        last_skeleton: None,
        last_mesh: None,
        last_images: Vec::new(),
        selected_image_tab: 0,
        texture_zoom: 1.0,
        texture_offset: [0.0, 0.0],
        texture_tiled: false,
        texture_tile_count: 3,
        gen_time_ms: 0.0,
        last_world: None,

        // Window / GPU
        args,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),

        // Input
        orbit: OrbitCameraController::new(),
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

        // Status message
        status_message: None,
    };
    app.orbit.auto_orbit = auto_orbit;

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
    last_skeleton: Option<flint_procgen::SkeletonDef>,
    last_mesh: Option<MeshData>,
    last_images: Vec<flint_procgen::ImageData>,
    selected_image_tab: usize,
    texture_zoom: f32,
    texture_offset: [f32; 2],
    texture_tiled: bool,
    texture_tile_count: usize,
    gen_time_ms: f64,
    last_world: Option<FlintWorld>,

    // Window / GPU
    args: GenPreviewArgs,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input
    orbit: OrbitCameraController,
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

    // Status message shown briefly in the UI after actions
    status_message: Option<(String, Instant)>,
}

// ─── Initialization ─────────────────────────────────────────────────────────

impl GenPreviewApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let title = format!("Flint Gen Preview \u{2014} {}", self.spec.meta.name);

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
                    self.last_skeleton = None;
                    self.last_images.clear();
                    self.lod_count = 1;
                    self.selected_lod = 0;
                }
                GeneratorOutput::MeshWithLods(lods) => {
                    self.last_skeleton = None;
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
                GeneratorOutput::SkinnedMesh(skinned) => {
                    // Store skeleton for visualization
                    self.last_skeleton = Some(skinned.skeleton.clone());

                    // Convert skinned mesh to regular MeshData for preview
                    // (renders the rest-pose geometry without skinning)
                    let mesh = flint_procgen::MeshData {
                        vertices: skinned
                            .vertices
                            .iter()
                            .map(|v| flint_procgen::Vertex {
                                position: v.position,
                                normal: v.normal,
                                tangent: v.tangent,
                                uv: v.uv,
                            })
                            .collect(),
                        indices: skinned.indices.clone(),
                        materials: skinned.materials.clone(),
                        submeshes: skinned.submeshes.clone(),
                        bounding_box: skinned.bounding_box,
                    };
                    self.upload_mesh(&mesh, 0);
                    self.last_mesh = Some(mesh);
                    self.last_images.clear();
                    self.lod_count = 1;
                    self.selected_lod = 0;
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

        // Refresh skeleton overlay if enabled
        self.update_skeleton_overlay();
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

        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.load_procedural_mesh(
                &context.device,
                "procgen_preview",
                &render_vertices,
                &mesh.indices,
                material,
            );
        }

        let world = build_mesh_world(1);
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.update_from_world(&world, &context.device);
        }
        self.last_world = Some(world);

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
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.update_from_world(&world, &context.device);
        }
        self.last_world = Some(world);

        self.auto_fit_to_mesh(mesh);
    }

    // mesh_world is now build_mesh_world() free function below

    fn auto_fit_to_mesh(&mut self, mesh: &MeshData) {
        let bb = &mesh.bounding_box;
        let center = bb.center();
        let size = bb.size();
        let diag = (size.x * size.x + size.y * size.y + size.z * size.z).sqrt();

        self.camera.target = Vec3::new(center.x, center.y, center.z);
        self.camera.distance = (diag * 1.2).max(2.0);
        self.camera.yaw = std::f32::consts::FRAC_PI_4;
        self.camera.pitch = 0.5;
        self.camera.update_orbit();
    }

    /// Update skeleton overlay from stored skeleton data.
    fn update_skeleton_overlay(&mut self) {
        let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer)
        else {
            return;
        };

        if !renderer.debug_state().show_skeleton {
            renderer.clear_skeleton_overlay();
            return;
        }

        let Some(skeleton) = &self.last_skeleton else {
            renderer.clear_skeleton_overlay();
            return;
        };

        let world_transforms = skeleton.compute_world_transforms();
        let positions: Vec<[f32; 3]> = world_transforms
            .iter()
            .map(|m| {
                // Extract translation from column-major 4x4 matrix (columns 12,13,14)
                [m[12], m[13], m[14]]
            })
            .collect();
        let parents: Vec<Option<usize>> = skeleton.bones.iter().map(|b| b.parent).collect();

        let mesh = flint_render::generate_skeleton_lines(&positions, &parents);
        renderer.set_skeleton_overlay(&context.device, &mesh);
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
            let handle =
                self.egui_ctx
                    .load_texture(&label, color_image, egui::TextureOptions::LINEAR);
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
        self.dirty && self.last_change.elapsed() > Duration::from_millis(self.debounce_ms)
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
                    self.set_status(&format!("Failed to save: {e}"));
                } else {
                    // Pause watcher to avoid reload loop
                    self.watcher_paused_until = Some(Instant::now() + Duration::from_millis(1000));
                    self.set_status(&format!("Saved {}", self.spec_path.display()));
                }
            }
            Err(e) => {
                self.set_status(&format!("Serialize failed: {e}"));
            }
        }
    }

    // ─── Export output ────────────────────────────────────────────────────

    fn export_output(&mut self) {
        // Determine default filename and extension from the spec name
        let base_name = self
            .spec
            .meta
            .name
            .replace(' ', "_")
            .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");

        let has_mesh = self.last_mesh.is_some();
        let has_images = !self.last_images.is_empty();

        if !has_mesh && !has_images {
            self.set_status("Nothing to export");
            return;
        }

        if has_mesh {
            let dialog = rfd::FileDialog::new()
                .set_title("Export GLB")
                .add_filter("glTF Binary", &["glb"])
                .set_file_name(format!("{base_name}.glb"));
            let dialog = if let Some(parent) = self.spec_path.parent() {
                dialog.set_directory(parent)
            } else {
                dialog
            };

            if let Some(path) = dialog.save_file() {
                self.export_mesh_to_path(&path);
            }
        } else if has_images {
            if self.last_images.len() == 1 {
                let dialog = rfd::FileDialog::new()
                    .set_title("Export PNG")
                    .add_filter("PNG Image", &["png"])
                    .set_file_name(format!("{base_name}.png"));
                let dialog = if let Some(parent) = self.spec_path.parent() {
                    dialog.set_directory(parent)
                } else {
                    dialog
                };

                if let Some(path) = dialog.save_file() {
                    self.export_image_to_path(&self.last_images[0].clone(), &path);
                }
            } else {
                // Multiple images: pick a directory, save with suffixed names
                let dialog = rfd::FileDialog::new()
                    .set_title("Export PNGs (select folder)")
                    .set_file_name(format!("{base_name}.png"));
                let dialog = if let Some(parent) = self.spec_path.parent() {
                    dialog.set_directory(parent)
                } else {
                    dialog
                };

                if let Some(base_path) = dialog.save_file() {
                    let stem = base_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("output");
                    let parent = base_path.parent().unwrap_or(Path::new("."));
                    let images = self.last_images.clone();
                    let mut count = 0;
                    for img in &images {
                        let suffix = format!("{:?}", img.channel_semantics).to_lowercase();
                        let img_path = parent.join(format!("{stem}_{suffix}.png"));
                        if let Err(e) = img.save_png(&img_path) {
                            tracing::error!("Failed to save {}: {}", img_path.display(), e);
                        } else {
                            count += 1;
                        }
                    }
                    self.set_status(&format!("Exported {count} PNGs to {}", parent.display()));
                }
            }
        }
    }

    fn export_mesh_to_path(&mut self, path: &Path) {
        // Re-generate to get the current output (need the full GeneratorOutput for LODs/skinned)
        self.spec.params = toml::Value::Table(self.params.clone());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };

        let result = self.registry.generate_from_spec(&self.spec);
        match result {
            Ok(GeneratorOutput::Mesh(mesh)) => match export_glb::mesh_to_glb(&mesh) {
                Ok(glb) => match std::fs::write(path, &glb) {
                    Ok(()) => self.set_status(&format!("Exported {}", path.display())),
                    Err(e) => self.set_status(&format!("Write failed: {e}")),
                },
                Err(e) => self.set_status(&format!("GLB export failed: {e}")),
            },
            Ok(GeneratorOutput::MeshWithLods(lods)) => match export_glb::mesh_lods_to_glb(&lods) {
                Ok(glb) => match std::fs::write(path, &glb) {
                    Ok(()) => self.set_status(&format!(
                        "Exported {} ({} LODs)",
                        path.display(),
                        lods.len()
                    )),
                    Err(e) => self.set_status(&format!("Write failed: {e}")),
                },
                Err(e) => self.set_status(&format!("GLB export failed: {e}")),
            },
            Ok(GeneratorOutput::SkinnedMesh(mesh)) => {
                match export_glb::skinned_mesh_to_glb(&mesh) {
                    Ok(glb) => match std::fs::write(path, &glb) {
                        Ok(()) => self.set_status(&format!("Exported {}", path.display())),
                        Err(e) => self.set_status(&format!("Write failed: {e}")),
                    },
                    Err(e) => self.set_status(&format!("GLB export failed: {e}")),
                }
            }
            Ok(_) => self.set_status("No mesh to export"),
            Err(e) => self.set_status(&format!("Generation failed: {e}")),
        }
    }

    fn export_image_to_path(&mut self, img: &flint_procgen::ImageData, path: &Path) {
        match img.save_png(path) {
            Ok(()) => self.set_status(&format!("Exported {}", path.display())),
            Err(e) => self.set_status(&format!("Save failed: {e}")),
        }
    }

    // ─── Save spec as ────────────────────────────────────────────────────

    fn save_spec_as(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Save Spec As")
            .add_filter("ProcGen Spec", &["toml"])
            .set_file_name(
                self.spec_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("spec.procgen.toml"),
            );
        let dialog = if let Some(parent) = self.spec_path.parent() {
            dialog.set_directory(parent)
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            self.spec.params = toml::Value::Table(self.params.clone());
            self.spec.seed = SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(self.current_seed),
                derive_from: None,
            };

            match toml::to_string_pretty(&self.spec) {
                Ok(content) => {
                    if let Err(e) = std::fs::write(&path, &content) {
                        self.set_status(&format!("Failed to save: {e}"));
                    } else {
                        // Update the spec path to the new location
                        self.spec_path = path.clone();
                        self.watcher_paused_until =
                            Some(Instant::now() + Duration::from_millis(1000));
                        self.set_status(&format!("Saved to {}", path.display()));
                    }
                }
                Err(e) => {
                    self.set_status(&format!("Serialize failed: {e}"));
                }
            }
        }
    }

    fn set_status(&mut self, msg: &str) {
        tracing::info!("{}", msg);
        self.status_message = Some((msg.to_string(), Instant::now()));
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
        let mut texture_zoom = self.texture_zoom;
        let mut texture_offset = self.texture_offset;
        let mut texture_tiled = self.texture_tiled;
        let mut texture_tile_count = self.texture_tile_count;
        let egui_textures = &self.egui_textures;
        let status_msg = self.status_message.clone();

        let mut dirty = false;
        let mut want_save = false;
        let mut want_save_as = false;
        let mut want_export = false;
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
                        if ui
                            .add(egui::DragValue::new(&mut seed_i64).speed(1))
                            .changed()
                        {
                            current_seed = seed_i64.max(0) as u64;
                            dirty = true;
                        }
                        if ui.button("Randomize").clicked() {
                            want_randomize = true;
                        }
                    });
                    ui.separator();

                    // Bottom section (stats, buttons, status) — always visible
                    egui::TopBottomPanel::bottom("param_panel_bottom").show_inside(ui, |ui| {
                        // LOD selector
                        if lod_count > 1 {
                            ui.horizontal(|ui| {
                                ui.label("LOD:");
                                for i in 0..lod_count {
                                    if ui
                                        .selectable_label(selected_lod == i, format!("{}", i))
                                        .clicked()
                                    {
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

                        ui.separator();

                        // Action buttons
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
                            if ui.button("Reset").clicked() {
                                want_reset = true;
                            }
                        });

                        // Status message (auto-fades after 4 seconds)
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

                    // Parameter groups — fills remaining space with scroll
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
                });

            // Texture display in central panel (only for image output)
            if is_texture && !egui_textures.is_empty() {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Toolbar: channel tabs + view controls
                    ui.horizontal(|ui| {
                        for (i, (label, _)) in egui_textures.iter().enumerate() {
                            if ui.selectable_label(selected_tab == i, label).clicked() {
                                selected_tab = i;
                            }
                        }
                        ui.separator();
                        ui.checkbox(&mut texture_tiled, "Tiled");
                        if texture_tiled {
                            for n in [2usize, 3, 4] {
                                if ui
                                    .selectable_label(texture_tile_count == n, format!("{n}x{n}"))
                                    .clicked()
                                {
                                    texture_tile_count = n;
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("\u{2212}").clicked() {
                            texture_zoom = (texture_zoom / 1.25).max(0.1);
                        }
                        ui.label(format!("{:.0}%", texture_zoom * 100.0));
                        if ui.button("+").clicked() {
                            texture_zoom = (texture_zoom * 1.25).min(20.0);
                        }
                        if ui.button("Fit").clicked() {
                            texture_zoom = 1.0;
                            texture_offset = [0.0, 0.0];
                        }
                    });
                    ui.separator();

                    // Display selected texture with zoom/pan/tiling
                    if let Some((_, handle)) = egui_textures.get(selected_tab) {
                        let tex_size = handle.size_vec2();
                        let available = ui.available_size();

                        // Fit scale: zoom 1.0 = content fits in viewport
                        let tiles_f = if texture_tiled {
                            texture_tile_count as f32
                        } else {
                            1.0
                        };
                        let fit_scale = (available.x / (tex_size.x * tiles_f))
                            .min(available.y / (tex_size.y * tiles_f));

                        // Interactive area for zoom/pan
                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());
                        let rect = response.rect;

                        // Scroll-to-zoom (centered on cursor)
                        if response.hovered() {
                            let scroll = ui.input(|i| i.raw_scroll_delta.y);
                            if scroll != 0.0 {
                                let old_zoom = texture_zoom;
                                texture_zoom =
                                    (texture_zoom * (1.0 + scroll * 0.003)).clamp(0.1, 20.0);
                                if let Some(mouse_pos) = response.hover_pos() {
                                    let factor = texture_zoom / old_zoom;
                                    let rel = mouse_pos - rect.center();
                                    texture_offset[0] =
                                        texture_offset[0] * factor + rel.x * (1.0 - factor);
                                    texture_offset[1] =
                                        texture_offset[1] * factor + rel.y * (1.0 - factor);
                                }
                            }
                        }

                        // Drag-to-pan
                        if response.dragged() {
                            let delta = response.drag_delta();
                            texture_offset[0] += delta.x;
                            texture_offset[1] += delta.y;
                        }

                        // Double-click to reset
                        if response.double_clicked() {
                            texture_zoom = 1.0;
                            texture_offset = [0.0, 0.0];
                        }

                        // Calculate tile size and draw
                        let tile_px = egui::vec2(
                            tex_size.x * fit_scale * texture_zoom,
                            tex_size.y * fit_scale * texture_zoom,
                        );
                        let total = egui::vec2(tile_px.x * tiles_f, tile_px.y * tiles_f);
                        let origin = rect.center()
                            + egui::vec2(texture_offset[0], texture_offset[1])
                            - total * 0.5;

                        let uv =
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        let tiles_n = if texture_tiled { texture_tile_count } else { 1 };
                        for ty in 0..tiles_n {
                            for tx in 0..tiles_n {
                                let pos = origin
                                    + egui::vec2(tx as f32 * tile_px.x, ty as f32 * tile_px.y);
                                let tile_rect = egui::Rect::from_min_size(pos, tile_px);
                                painter.image(handle.id(), tile_rect, uv, egui::Color32::WHITE);
                            }
                        }
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
        self.texture_zoom = texture_zoom;
        self.texture_offset = texture_offset;
        self.texture_tiled = texture_tiled;
        self.texture_tile_count = texture_tile_count;
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
            self.export_output();
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
                        renderer.resize_postprocess(
                            &ctx.device,
                            physical_size.width,
                            physical_size.height,
                        );
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Let orbit controller track WASD/QE held state
                self.orbit.handle_key_event(&event);

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
                        PhysicalKey::Code(KeyCode::KeyS)
                            if !event.repeat && event.state == ElementState::Pressed =>
                        {
                            let mods = self.egui_ctx.input(|i| i.modifiers);
                            if mods.command && mods.shift {
                                // Save As (Ctrl+Shift+S)
                                self.save_spec_as();
                            } else if mods.command {
                                // Save spec (Ctrl+S)
                                self.save_spec();
                            }
                        }
                        PhysicalKey::Code(KeyCode::KeyE)
                            if !event.repeat && event.state == ElementState::Pressed =>
                        {
                            // Export output (Ctrl+E)
                            if self.egui_ctx.input(|i| i.modifiers.command) {
                                self.export_output();
                            }
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            // Reset camera / texture view
                            if let Some(mesh) = &self.last_mesh {
                                let mesh_clone = mesh.clone();
                                self.auto_fit_to_mesh(&mesh_clone);
                            }
                            self.texture_zoom = 1.0;
                            self.texture_offset = [0.0, 0.0];
                        }
                        PhysicalKey::Code(KeyCode::KeyT) => {
                            if !self.last_images.is_empty() {
                                self.texture_tiled = !self.texture_tiled;
                            }
                        }
                        PhysicalKey::Code(KeyCode::F1) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let next = renderer.debug_state().mode.next();
                                renderer.set_debug_mode(next);
                                println!("Debug mode: {}", next.label());
                                if let (Some(world), Some(context)) =
                                    (&self.last_world, &self.render_context)
                                {
                                    renderer.update_from_world(world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F2) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let mode =
                                    if renderer.debug_state().mode == DebugMode::WireframeOverlay {
                                        DebugMode::Pbr
                                    } else {
                                        DebugMode::WireframeOverlay
                                    };
                                renderer.set_debug_mode(mode);
                                println!("Debug mode: {}", mode.label());
                                if let (Some(world), Some(context)) =
                                    (&self.last_world, &self.render_context)
                                {
                                    renderer.update_from_world(world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F3) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_normal_arrows();
                                println!("Normal arrows: {}", if on { "ON" } else { "OFF" });
                                if let (Some(world), Some(context)) =
                                    (&self.last_world, &self.render_context)
                                {
                                    renderer.update_from_world(world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F4) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_skeleton_overlay();
                                println!("Skeleton overlay: {}", if on { "ON" } else { "OFF" });
                            }
                            self.update_skeleton_overlay();
                        }
                        PhysicalKey::Code(KeyCode::KeyO) => {
                            self.orbit.auto_orbit = !self.orbit.auto_orbit;
                            println!(
                                "Auto-orbit: {}",
                                if self.orbit.auto_orbit { "ON" } else { "OFF" }
                            );
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            self.orbit.adjust_auto_orbit_speed(1.5);
                            println!("Auto-orbit speed: {:.2} rad/s", self.orbit.auto_orbit_speed);
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) => {
                            self.orbit.adjust_auto_orbit_speed(1.0 / 1.5);
                            println!("Auto-orbit speed: {:.2} rad/s", self.orbit.auto_orbit_speed);
                        }
                        _ => {}
                    }
                }
            }

            ref ev @ (WindowEvent::MouseInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseWheel { .. }) => {
                self.orbit
                    .handle_event(ev, &mut self.camera, self.egui_ctx.is_pointer_over_area());
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self.last_frame_time.elapsed().as_secs_f32();
                self.last_frame_time = now;

                // Apply held keyboard orbit/zoom
                self.orbit.update(&mut self.camera, dt);

                // Texture zoom via Q/E keys (same keys as 3D zoom)
                if !self.last_images.is_empty() {
                    let zoom_speed = 2.0;
                    if self.orbit.is_key_held(KeyCode::KeyE) {
                        self.texture_zoom = (self.texture_zoom * (1.0 + zoom_speed * dt)).min(20.0);
                    }
                    if self.orbit.is_key_held(KeyCode::KeyQ) {
                        self.texture_zoom = (self.texture_zoom * (1.0 - zoom_speed * dt)).max(0.1);
                    }
                }

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
                    let mut encoder =
                        context
                            .device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("Clear Encoder"),
                            });
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
            m.insert("asset".to_string(), toml::Value::String(asset_name));
            m
        });
        let _ = world.set_component(eid, comp::MODEL, model);
    }

    world
}

// ─── Parameter UI helpers (delegated to gen_preview_common) ─────────────────

use super::gen_preview_common::{format_count, group_fields, random_seed, render_param_field};
