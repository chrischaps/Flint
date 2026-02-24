//! Preview a 3D model file (GLB/glTF) with orbit camera
//!
//! Three modes:
//! 1. `flint preview model.glb` — interactive window with orbit camera
//! 2. `flint preview` — empty window, drag-and-drop a .glb/.gltf to load
//! 3. `flint preview model.glb --render out.png` — headless render to PNG

use anyhow::{Context, Result};
use flint_animation::node_clip::NodeClip;
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_import::{import_gltf, ImportResult, MeshBounds};
use flint_render::model_loader::{self, ModelLoadConfig, ModelLoadResult};
use flint_render::{Camera, DebugMode, HeadlessContext, RendererConfig, SceneRenderer};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

pub struct PreviewArgs {
    pub model: Option<String>,
    pub render: Option<String>,
    pub width: u32,
    pub height: u32,
    pub distance: Option<f32>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub target: Option<[f32; 3]>,
    pub fov: Option<f32>,
    pub no_grid: bool,
    pub watch: bool,
    pub no_animate: bool,
    pub clip: Option<String>,
    pub anim_speed: f32,
    pub anim_time: Option<f32>,
}

pub fn run(args: PreviewArgs) -> Result<()> {
    if let Some(render_output) = &args.render {
        run_headless(&args, render_output)
    } else {
        run_interactive(args)
    }
}

// ---------------------------------------------------------------------------
// Animation info tracked alongside the preview
// ---------------------------------------------------------------------------

struct AnimationInfo {
    clip_names: Vec<String>,
    current_clip_index: usize,
}

// ---------------------------------------------------------------------------
// Model statistics for UI display
// ---------------------------------------------------------------------------

struct ModelStats {
    total_vertices: usize,
    total_triangles: usize,
    mesh_count: usize,
    material_count: usize,
    node_count: usize,
    skeleton_joint_count: usize,
    bounds: Option<MeshBounds>,
}

impl ModelStats {
    fn from_import(import: &ImportResult) -> Self {
        let total_vertices: usize = import.meshes.iter().map(|m| m.positions.len()).sum();
        let total_triangles: usize = import
            .meshes
            .iter()
            .map(|m| m.indices.len() / 3)
            .sum();
        let skeleton_joint_count: usize = import
            .skeletons
            .iter()
            .map(|s| s.joints.len())
            .sum();
        Self {
            total_vertices,
            total_triangles,
            mesh_count: import.meshes.len(),
            material_count: import.materials.len(),
            node_count: import.nodes.len(),
            skeleton_joint_count,
            bounds: import.bounds(),
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal ECS world for a single model
// ---------------------------------------------------------------------------

/// Create an ECS world with one entity at origin containing transform + model + animator components.
/// Returns (world, asset_name, entity_id).
fn create_model_world(model_path: &Path, anim_speed: f32) -> (FlintWorld, String, EntityId) {
    let asset_name = model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();

    let mut world = FlintWorld::new();
    let entity_id = world.spawn("preview_model").expect("spawn entity");

    // Transform at origin
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
    let _ = world.set_component(entity_id, "transform", transform);

    // Model component pointing to asset name
    let model = toml::Value::Table({
        let mut m = toml::map::Map::new();
        m.insert("asset".to_string(), toml::Value::String(asset_name.clone()));
        m
    });
    let _ = world.set_component(entity_id, "model", model);

    // Animator component — enables animated model expansion and skeletal sync discovery
    let animator = toml::Value::Table({
        let mut a = toml::map::Map::new();
        a.insert("clip".to_string(), toml::Value::String(String::new()));
        a.insert("playing".to_string(), toml::Value::Boolean(true));
        a.insert("loop".to_string(), toml::Value::Boolean(true));
        a.insert(
            "speed".to_string(),
            toml::Value::Float(anim_speed as f64),
        );
        a
    });
    let _ = world.set_component(entity_id, "animator", animator);

    (world, asset_name, entity_id)
}

/// Build a ModelLoadConfig with an override so the asset name resolves to the exact file path.
fn model_load_config(model_path: &Path, asset_name: &str) -> ModelLoadConfig {
    let scene_dir = model_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut overrides = HashMap::new();
    overrides.insert(asset_name.to_string(), model_path.to_path_buf());
    ModelLoadConfig {
        scene_dir,
        overrides,
    }
}

// ---------------------------------------------------------------------------
// Animation registration from model load results
// ---------------------------------------------------------------------------

/// Register skeletal and node animation data from loaded models into the animation system.
/// Returns AnimationInfo if any clips were found, and the skeletal entity→asset map for bone upload.
fn register_animation_data(
    load_result: &ModelLoadResult,
    animation: &mut AnimationSystem,
    world: &mut FlintWorld,
    entity_id: EntityId,
    requested_clip: Option<&str>,
    anim_speed: f32,
) -> (Option<AnimationInfo>, HashMap<EntityId, String>) {
    let mut all_clip_names: Vec<String> = Vec::new();
    let mut skeletal_entity_assets: HashMap<EntityId, String> = HashMap::new();

    for loaded in &load_result.models {
        // Skeletal animation
        if loaded.is_skinned {
            if let Some(ref import_result) = loaded.import_result {
                for imported_skel in &import_result.skeletons {
                    let skeleton = Skeleton::from_imported(imported_skel);
                    animation
                        .skeletal_sync
                        .add_skeleton(loaded.entity_id, skeleton);
                }
                for imported_clip in &import_result.skeletal_clips {
                    let clip = SkeletalClip::from_imported(imported_clip);
                    println!(
                        "  Skeletal clip: {} ({:.1}s, {} tracks)",
                        clip.name, clip.duration, clip.joint_tracks.len()
                    );

                    all_clip_names.push(clip.name.clone());
                    animation.skeletal_sync.add_clip(clip);
                }

                // Add skeleton component so SkeletalSync::sync_from_world discovers this entity
                let skeleton_comp = toml::Value::Table({
                    let mut s = toml::map::Map::new();
                    s.insert("skin".to_string(), toml::Value::String(String::new()));
                    s
                });
                let _ = world.set_component(loaded.entity_id, "skeleton", skeleton_comp);

                skeletal_entity_assets
                    .insert(loaded.entity_id, loaded.asset_name.clone());
            }
        }

        // Node animation
        if let Some(ref import_result) = loaded.import_result {
            for imported_clip in &import_result.node_clips {
                let clip = NodeClip::from_imported(imported_clip);
                println!(
                    "  Node clip: {} ({:.1}s, {} tracks)",
                    clip.name, clip.duration, clip.node_tracks.len()
                );

                all_clip_names.push(clip.name.clone());
                animation.node_sync.add_clip(clip);
            }
        }
        if let Some(ref node_map) = loaded.node_map {
            animation
                .node_sync
                .register_entity(loaded.entity_id, node_map.clone());
        }
    }

    // Deduplicate (skeletal and node clips could share names, though unlikely)
    all_clip_names.sort();
    all_clip_names.dedup();

    if all_clip_names.is_empty() {
        return (None, skeletal_entity_assets);
    }

    // Determine which clip to start with
    let current_clip_index = if let Some(requested) = requested_clip {
        all_clip_names
            .iter()
            .position(|n| n == requested)
            .unwrap_or_else(|| {
                eprintln!(
                    "Clip '{}' not found; available: {}",
                    requested,
                    all_clip_names.join(", ")
                );
                0
            })
    } else {
        0
    };

    let clip_name = &all_clip_names[current_clip_index];

    // Set the clip on the animator component so sync_from_world picks it up
    if let Some(components) = world.get_components_mut(entity_id) {
        components.set_field(
            "animator",
            "clip",
            toml::Value::String(clip_name.clone()),
        );
        components.set_field("animator", "playing", toml::Value::Boolean(true));
        components.set_field(
            "animator",
            "speed",
            toml::Value::Float(anim_speed as f64),
        );
    }

    let info = AnimationInfo {
        clip_names: all_clip_names,
        current_clip_index,
    };

    (Some(info), skeletal_entity_assets)
}

// ---------------------------------------------------------------------------
// Auto-fit camera to model bounds
// ---------------------------------------------------------------------------

fn auto_fit_camera(bounds: &MeshBounds, camera: &mut Camera) {
    let center = [
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        (bounds.min[2] + bounds.max[2]) * 0.5,
    ];
    let size = bounds.size();
    let diagonal = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();

    camera.target = Vec3::new(center[0], center[1], center[2]);
    camera.distance = (diagonal * 1.2).max(2.0);
    camera.yaw = std::f32::consts::FRAC_PI_4; // 45 degrees
    camera.pitch = 0.5; // ~30 degrees
}

fn apply_cli_overrides(camera: &mut Camera, args: &PreviewArgs) {
    if let Some(d) = args.distance {
        camera.distance = d;
    }
    if let Some(y) = args.yaw {
        camera.yaw = y.to_radians();
    }
    if let Some(p) = args.pitch {
        camera.pitch = p.to_radians();
    }
    if let Some(t) = args.target {
        camera.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(f) = args.fov {
        camera.fov = f;
    }
}

// ---------------------------------------------------------------------------
// Headless render mode
// ---------------------------------------------------------------------------

fn run_headless(args: &PreviewArgs, output_path: &str) -> Result<()> {
    let model_str = args
        .model
        .as_deref()
        .context("A model path is required for headless rendering (--render)")?;
    let model_path = PathBuf::from(model_str);

    if !model_path.exists() {
        anyhow::bail!("Model file not found: {}", model_path.display());
    }

    // Import to get bounds for auto-fit
    let import_result =
        import_gltf(&model_path).context("Failed to import model for bounds computation")?;
    let bounds = import_result.bounds();

    // Headless context
    let ctx = pollster::block_on(HeadlessContext::new(args.width, args.height))
        .context("Failed to create headless render context")?;

    // Camera
    let mut camera = Camera::new();
    camera.aspect = ctx.aspect_ratio();
    if let Some(b) = &bounds {
        auto_fit_camera(b, &mut camera);
    }
    apply_cli_overrides(&mut camera, args);
    camera.update_orbit();

    // Renderer
    let mut renderer = SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        ctx.width,
        ctx.height,
        RendererConfig {
            show_grid: !args.no_grid,
        },
    );

    // Build world and load model
    let (mut world, asset_name, entity_id) = create_model_world(&model_path, args.anim_speed);
    let config = model_load_config(&model_path, &asset_name);
    let load_result = model_loader::load_models_from_world(
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
        &config,
    );

    // Animation support for --anim-time
    if let Some(anim_time) = args.anim_time {
        if !args.no_animate {
            let mut animation = AnimationSystem::new();
            let (anim_info, skeletal_entity_assets) = register_animation_data(
                &load_result,
                &mut animation,
                &mut world,
                entity_id,
                args.clip.as_deref(),
                args.anim_speed,
            );

            if anim_info.is_some() {
                // Sync and advance to the requested time
                animation.sync.sync_from_world(&world, &animation.player);
                animation.skeletal_sync.sync_from_world(&world);
                animation.node_sync.sync_from_world(&world);

                // Use update() which handles all three tiers
                let _ = flint_runtime::RuntimeSystem::update(
                    &mut animation,
                    &mut world,
                    anim_time as f64,
                );

                // Upload bone matrices
                for (eid, asset) in &skeletal_entity_assets {
                    if let Some(matrices) = animation.skeletal_sync.bone_matrices(eid) {
                        renderer.update_bone_matrices(&ctx.queue, asset, matrices);
                    }
                }

                println!("Sampled animation at t={:.3}s", anim_time);
            }
        }
    }

    renderer.update_from_world(&world, &ctx.device);

    // Render
    renderer.render_to(
        &ctx.device,
        &ctx.queue,
        &ctx.depth_view,
        &camera,
        &ctx.color_view,
    );

    // Read pixels and save
    let pixels = pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
    let img = image::RgbaImage::from_raw(args.width, args.height, pixels)
        .context("Failed to create image from pixel data")?;
    img.save(output_path)
        .context(format!("Failed to save image to {}", output_path))?;

    println!(
        "Rendered {}x{} preview to {}",
        args.width, args.height, output_path
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive windowed mode
// ---------------------------------------------------------------------------

struct PreviewState {
    world: FlintWorld,
    asset_name: String,
    model_path: Option<PathBuf>,
    has_model: bool,
    needs_reload: bool,
    entity_id: Option<EntityId>,
    import_result: Option<ImportResult>,
    model_stats: Option<ModelStats>,
}

fn run_interactive(args: PreviewArgs) -> Result<()> {
    let mut state = PreviewState {
        world: FlintWorld::new(),
        asset_name: String::new(),
        model_path: None,
        has_model: false,
        needs_reload: false,
        entity_id: None,
        import_result: None,
        model_stats: None,
    };

    // If a model path is provided, create the initial world
    let initial_bounds;
    if let Some(model_str) = &args.model {
        let model_path = PathBuf::from(model_str);
        if !model_path.exists() {
            anyhow::bail!("Model file not found: {}", model_path.display());
        }
        let (world, asset_name, entity_id) = create_model_world(&model_path, args.anim_speed);
        // Import to get bounds and cache the result for UI display
        let import = import_gltf(&model_path).ok();
        initial_bounds = import.as_ref().and_then(|r| r.bounds());
        if let Some(ref ir) = import {
            state.model_stats = Some(ModelStats::from_import(ir));
        }
        state.import_result = import;
        state.world = world;
        state.asset_name = asset_name;
        state.model_path = Some(model_path);
        state.has_model = true;
        state.entity_id = Some(entity_id);
    } else {
        initial_bounds = None;
    }

    let state = Arc::new(Mutex::new(state));

    // File watcher for --watch mode
    let _watcher = if args.watch {
        if let Some(model_str) = &args.model {
            let state_clone = Arc::clone(&state);
            let (tx, rx) = std::sync::mpsc::channel();
            let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
            let model_file = Path::new(model_str.as_str());

            debouncer
                .watcher()
                .watch(model_file, RecursiveMode::NonRecursive)?;

            std::thread::spawn(move || {
                for result in rx {
                    match result {
                        Ok(_events) => {
                            if let Ok(mut s) = state_clone.lock() {
                                s.needs_reload = true;
                            }
                        }
                        Err(e) => {
                            eprintln!("Watch error: {:?}", e);
                        }
                    }
                }
            });

            println!("Watching for changes...");
            Some(debouncer)
        } else {
            None
        }
    } else {
        None
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = PreviewApp {
        state,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),
        initial_bounds,
        args,

        mouse_pressed: false,
        right_mouse_pressed: false,
        last_mouse_pos: None,
        last_frame_time: Instant::now(),

        // Animation
        animation: AnimationSystem::new(),
        skeletal_entity_assets: HashMap::new(),
        anim_info: None,
        anim_paused: false,
        anim_time_accumulator: 0.0,

        // egui overlay
        egui_ctx: egui::Context::default(),
        egui_winit: None,
        egui_renderer: None,
        show_ui: true,

        // FPS tracking
        frame_times: VecDeque::new(),
        fps: 0.0,
        last_fps_update: Instant::now(),
    };

    event_loop.run_app(&mut app)?;

    Ok(())
}

struct PreviewApp {
    state: Arc<Mutex<PreviewState>>,
    window: Option<Arc<Window>>,
    render_context: Option<flint_render::RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,
    initial_bounds: Option<MeshBounds>,
    args: PreviewArgs,

    // Input
    mouse_pressed: bool,
    right_mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    last_frame_time: Instant,

    // Animation
    animation: AnimationSystem,
    skeletal_entity_assets: HashMap<EntityId, String>,
    anim_info: Option<AnimationInfo>,
    anim_paused: bool,
    /// Accumulated playback time for window title display
    anim_time_accumulator: f64,

    // egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_ui: bool,

    // FPS tracking
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_fps_update: Instant,
}

impl PreviewApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let has_model = self.state.lock().map(|s| s.has_model).unwrap_or(false);
        let model_name = self
            .state
            .lock()
            .ok()
            .and_then(|s| {
                s.model_path
                    .as_ref()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            })
            .unwrap_or_default();

        let title = if has_model {
            format!("Flint Preview \u{2014} {}", model_name)
        } else {
            "Flint Preview \u{2014} Drop a .glb/.gltf file".to_string()
        };

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&title)
                        .with_inner_size(PhysicalSize::new(self.args.width, self.args.height)),
                )
                .expect("Failed to create window"),
        );

        let context = pollster::block_on(flint_render::RenderContext::new(window.clone()))
            .expect("Failed to create render context");

        self.camera.aspect = context.aspect_ratio();

        let mut renderer = SceneRenderer::new(
            &context,
            RendererConfig {
                show_grid: !self.args.no_grid,
            },
        );

        // Load model if we have one
        if has_model {
            let state = self.state.lock().unwrap();
            let maybe_path = state.model_path.clone();
            let asset_name = state.asset_name.clone();
            let entity_id = state.entity_id;
            drop(state);

            if let Some(model_path) = maybe_path {
                let config = model_load_config(&model_path, &asset_name);
                let mut state = self.state.lock().unwrap();
                let load_result = model_loader::load_models_from_world(
                    &mut state.world,
                    &mut renderer,
                    &context.device,
                    &context.queue,
                    &config,
                );

                // Register animation data
                if !self.args.no_animate {
                    if let Some(eid) = entity_id {
                        let (info, skel_assets) = register_animation_data(
                            &load_result,
                            &mut self.animation,
                            &mut state.world,
                            eid,
                            self.args.clip.as_deref(),
                            self.args.anim_speed,
                        );
                        self.anim_info = info;
                        self.skeletal_entity_assets = skel_assets;
                    }
                }
            }

            // Auto-fit camera
            if let Some(b) = &self.initial_bounds {
                auto_fit_camera(b, &mut self.camera);
            }
            apply_cli_overrides(&mut self.camera, &self.args);
            self.camera.update_orbit();
        }

        // Initialize egui
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &context.device,
            context.config.format,
            None,
            1,
            false,
        );
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        self.window = Some(window);
        self.render_context = Some(context);
        self.scene_renderer = Some(renderer);

        if let Some(info) = &self.anim_info {
            println!(
                "Animation: {} clip(s) found — {}",
                info.clip_names.len(),
                info.clip_names.join(", ")
            );
            println!("  Tab=toggle UI  P=play/pause  [/]=prev/next clip  +/-=speed  0=reset speed");
        }
    }

    fn setup_animation_after_load(&mut self, load_result: &ModelLoadResult) {
        self.animation.clear();
        self.skeletal_entity_assets.clear();
        self.anim_info = None;
        self.anim_paused = false;
        self.anim_time_accumulator = 0.0;

        if self.args.no_animate {
            return;
        }

        let mut state = self.state.lock().unwrap();
        if let Some(eid) = state.entity_id {
            let (info, skel_assets) = register_animation_data(
                load_result,
                &mut self.animation,
                &mut state.world,
                eid,
                None, // No clip preference on reload/drop
                self.args.anim_speed,
            );
            self.anim_info = info;
            self.skeletal_entity_assets = skel_assets;
        }
    }

    fn load_model_file(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext != "glb" && ext != "gltf" {
            eprintln!("Unsupported file type: .{} (expected .glb or .gltf)", ext);
            return;
        }

        // Import to get bounds and cache for UI
        let import = match import_gltf(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to import model: {:?}", e);
                return;
            }
        };
        let bounds = import.bounds();
        let stats = ModelStats::from_import(&import);

        let (world, asset_name, entity_id) = create_model_world(&path, self.args.anim_speed);

        // Clear and reload
        let load_result;
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.clear_model_data();

            if let Some(context) = &self.render_context {
                let config = model_load_config(&path, &asset_name);
                let mut state = self.state.lock().unwrap();
                state.world = world;
                state.asset_name = asset_name;
                state.has_model = true;
                state.model_path = Some(path.clone());
                state.entity_id = Some(entity_id);
                state.model_stats = Some(stats);
                state.import_result = Some(import);

                load_result = Some(model_loader::load_models_from_world(
                    &mut state.world,
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                ));
            } else {
                load_result = None;
            }
        } else {
            load_result = None;
        }

        // Register animation data
        if let Some(lr) = &load_result {
            self.setup_animation_after_load(lr);
        }

        // Auto-fit camera
        if let Some(b) = &bounds {
            auto_fit_camera(b, &mut self.camera);
        }
        self.camera.update_orbit();

        // Update window title
        if let Some(window) = &self.window {
            let file_name = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            window.set_title(&format!("Flint Preview \u{2014} {}", file_name));
        }

        println!("Loaded: {}", path.display());
        if let Some(info) = &self.anim_info {
            println!(
                "Animation: {} clip(s) — {}",
                info.clip_names.len(),
                info.clip_names.join(", ")
            );
        }
    }

    fn check_reload(&mut self) {
        let needs_reload = self.state.lock().map(|s| s.needs_reload).unwrap_or(false);

        if !needs_reload {
            return;
        }

        // Phase 1: Import model, create world, load into renderer (holds state lock)
        let (load_result, bounds, model_display) = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.needs_reload = false;
            let Some(model_path) = state.model_path.clone() else {
                return;
            };

            let import = match import_gltf(&model_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Reload failed: {:?}", e);
                    return;
                }
            };
            let bounds = import.bounds();
            state.model_stats = Some(ModelStats::from_import(&import));
            state.import_result = Some(import);

            let (world, asset_name, entity_id) =
                create_model_world(&model_path, self.args.anim_speed);
            state.world = world;
            state.asset_name = asset_name.clone();
            state.entity_id = Some(entity_id);

            let load_result = if let (Some(renderer), Some(context)) =
                (&mut self.scene_renderer, &self.render_context)
            {
                renderer.clear_model_data();
                let config = model_load_config(&model_path, &asset_name);
                Some(model_loader::load_models_from_world(
                    &mut state.world,
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                ))
            } else {
                None
            };

            let display = model_path.display().to_string();
            (load_result, bounds, display)
        };
        // State lock is now dropped

        // Phase 2: Register animation data (acquires state lock internally)
        if let Some(lr) = &load_result {
            self.setup_animation_after_load(lr);
        }

        if let Some(b) = &bounds {
            auto_fit_camera(b, &mut self.camera);
        }
        self.camera.update_orbit();

        println!("Reloaded: {}", model_display);
    }

    /// Switch to a different animation clip by index
    fn switch_clip(&mut self, new_index: usize) {
        let info = match &mut self.anim_info {
            Some(i) => i,
            None => return,
        };
        if new_index >= info.clip_names.len() {
            return;
        }

        info.current_clip_index = new_index;
        let clip_name = info.clip_names[new_index].clone();

        // Reset accumulated time
        self.anim_time_accumulator = 0.0;

        // Update animator component in ECS so sync_from_world picks up the change
        if let Ok(mut state) = self.state.lock() {
            if let Some(eid) = state.entity_id {
                if let Some(components) = state.world.get_components_mut(eid) {
                    components.set_field(
                        "animator",
                        "clip",
                        toml::Value::String(clip_name.clone()),
                    );
                    components.set_field("animator", "playing", toml::Value::Boolean(true));
                }

                // Reset skeletal playback state so it re-syncs with the new clip
                self.animation.skeletal_sync.reset_state(&eid);
            }
        }

        println!("Switched to clip: {}", clip_name);
    }

    /// Update the window title with animation playback info
    fn update_title(&self) {
        let window = match &self.window {
            Some(w) => w,
            None => return,
        };

        let model_name = self
            .state
            .lock()
            .ok()
            .and_then(|s| {
                s.model_path
                    .as_ref()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            })
            .unwrap_or_default();

        if let Some(info) = &self.anim_info {
            let clip_name = &info.clip_names[info.current_clip_index];
            let status = if self.anim_paused { "\u{23f8}" } else { "\u{25b6}" };
            let speed = self
                .state
                .lock()
                .ok()
                .and_then(|s| {
                    s.entity_id.and_then(|eid| {
                        s.world.get_components(eid).and_then(|c| {
                            c.get("animator").and_then(|a| {
                                a.get("speed")
                                    .and_then(|v| {
                                        v.as_float()
                                            .or_else(|| v.as_integer().map(|i| i as f64))
                                    })
                            })
                        })
                    })
                })
                .unwrap_or(1.0);

            let title = format!(
                "Flint Preview \u{2014} {} | {} {} ({}/{}) [{:.1}x]",
                model_name,
                status,
                clip_name,
                info.current_clip_index + 1,
                info.clip_names.len(),
                speed,
            );
            window.set_title(&title);
        } else {
            let title = if model_name.is_empty() {
                "Flint Preview \u{2014} Drop a .glb/.gltf file".to_string()
            } else {
                format!("Flint Preview \u{2014} {}", model_name)
            };
            window.set_title(&title);
        }
    }

    // -----------------------------------------------------------------------
    // egui overlay rendering
    // -----------------------------------------------------------------------

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
            Some(e) => e,
            None => return,
        };

        let raw_input = egui_winit.take_egui_input(&window);

        // Snapshot renderer state before closure (avoid borrow conflicts)
        let current_debug_mode = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().mode)
            .unwrap_or_default();
        let current_wireframe = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().wireframe_overlay)
            .unwrap_or(false);
        let current_normals = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().show_normals)
            .unwrap_or(false);
        let current_grid = self
            .scene_renderer
            .as_ref()
            .map(|r| r.show_grid())
            .unwrap_or(false);

        // Snapshot camera info
        let cam_distance = self.camera.distance;
        let cam_yaw_deg = self.camera.yaw.to_degrees();
        let cam_pitch_deg = self.camera.pitch.to_degrees();
        let fps = self.fps;

        // Snapshot animation state
        let anim_paused = self.anim_paused;
        let anim_clip_names: Vec<String> = self
            .anim_info
            .as_ref()
            .map(|i| i.clip_names.clone())
            .unwrap_or_default();
        let anim_clip_index = self
            .anim_info
            .as_ref()
            .map(|i| i.current_clip_index)
            .unwrap_or(0);
        let has_anim = self.anim_info.is_some();

        // Get playback time and clip duration from animation system
        let (anim_time, anim_duration, anim_speed) = if has_anim {
            let state_guard = self.state.lock().ok();
            let entity_id = state_guard
                .as_ref()
                .and_then(|s| s.entity_id);

            let mut time = 0.0f64;
            let mut duration = 0.0f64;
            let mut speed = 1.0f64;

            if let Some(eid) = entity_id {
                // Try skeletal sync first
                if let Some(ps) = self.animation.skeletal_sync.get_playback_state(&eid) {
                    time = ps.time;
                    speed = ps.speed;
                }
                // Try node sync
                if let Some(ps) = self.animation.node_sync.get_playback_state(&eid) {
                    time = ps.time;
                    speed = ps.speed;
                }
                // Get duration from current clip name
                if !anim_clip_names.is_empty() {
                    let clip_name = &anim_clip_names[anim_clip_index];
                    if let Some(d) = self.animation.skeletal_sync.get_clip_duration(clip_name) {
                        duration = d;
                    }
                    if let Some(d) = self.animation.node_sync.get_clip_duration(clip_name) {
                        duration = d;
                    }
                }
            }
            (time, duration, speed)
        } else {
            (0.0, 0.0, 1.0)
        };

        // Read model stats and node/material info from state
        let state_guard = self.state.lock().ok();
        let model_stats_snapshot = state_guard.as_ref().and_then(|s| s.model_stats.as_ref()).map(
            |ms| {
                (
                    ms.total_vertices,
                    ms.total_triangles,
                    ms.mesh_count,
                    ms.material_count,
                    ms.node_count,
                    ms.skeleton_joint_count,
                    ms.bounds,
                )
            },
        );
        // Snapshot node tree data
        let node_data: Vec<(String, Vec<usize>, bool)> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| {
                ir.nodes
                    .iter()
                    .map(|n| {
                        (
                            n.name.clone(),
                            n.children.clone(),
                            !n.mesh_primitive_indices.is_empty(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let root_nodes: Vec<usize> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| ir.root_nodes.clone())
            .unwrap_or_default();
        // Snapshot material data
        let material_data: Vec<(String, [f32; 4], f32, f32)> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| {
                ir.materials
                    .iter()
                    .map(|m| (m.name.clone(), m.base_color, m.metallic, m.roughness))
                    .collect()
            })
            .unwrap_or_default();
        let has_model = state_guard.as_ref().map(|s| s.has_model).unwrap_or(false);
        drop(state_guard);

        // Mutation requests collected from UI interactions
        let mut new_debug_mode: Option<DebugMode> = None;
        let mut new_wireframe: Option<bool> = None;
        let mut new_normals: Option<bool> = None;
        let mut new_grid: Option<bool> = None;
        let mut new_anim_paused: Option<bool> = None;
        let mut new_clip_index: Option<usize> = None;
        let mut new_speed: Option<f64> = None;
        let mut scrub_time: Option<f64> = None;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // ── Left panel: model info ──
            egui::SidePanel::left("preview_info_panel")
                .default_width(220.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading("Model Inspector");
                    ui.separator();

                    if !has_model {
                        ui.label("No model loaded");
                        ui.label("Drop a .glb/.gltf file to preview");
                        return;
                    }

                    // ── Statistics ──
                    egui::CollapsingHeader::new("Statistics")
                        .default_open(true)
                        .show(ui, |ui| {
                            if let Some((verts, tris, meshes, mats, nodes, joints, bounds)) =
                                model_stats_snapshot
                            {
                                egui::Grid::new("stats_grid")
                                    .num_columns(2)
                                    .spacing([8.0, 2.0])
                                    .show(ui, |ui| {
                                        ui.label("Meshes:");
                                        ui.label(format!("{}", meshes));
                                        ui.end_row();
                                        ui.label("Vertices:");
                                        ui.label(format!("{}", verts));
                                        ui.end_row();
                                        ui.label("Triangles:");
                                        ui.label(format!("{}", tris));
                                        ui.end_row();
                                        ui.label("Materials:");
                                        ui.label(format!("{}", mats));
                                        ui.end_row();
                                        ui.label("Nodes:");
                                        ui.label(format!("{}", nodes));
                                        ui.end_row();
                                        if joints > 0 {
                                            ui.label("Joints:");
                                            ui.label(format!("{}", joints));
                                            ui.end_row();
                                        }
                                    });

                                if let Some(b) = bounds {
                                    let size = b.size();
                                    ui.add_space(4.0);
                                    ui.label(format!(
                                        "Size: {:.2} x {:.2} x {:.2}",
                                        size[0], size[1], size[2]
                                    ));
                                }
                            }
                        });

                    ui.add_space(4.0);

                    // ── Node Hierarchy ──
                    if !node_data.is_empty() {
                        egui::CollapsingHeader::new("Node Hierarchy")
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        for &root_idx in &root_nodes {
                                            render_node_tree(ui, &node_data, root_idx, 0);
                                        }
                                    });
                            });

                        ui.add_space(4.0);
                    }

                    // ── Materials ──
                    if !material_data.is_empty() {
                        egui::CollapsingHeader::new("Materials")
                            .default_open(false)
                            .show(ui, |ui| {
                                for (i, (name, color, metallic, roughness)) in
                                    material_data.iter().enumerate()
                                {
                                    let display_name = if name.is_empty() {
                                        format!("Material {}", i)
                                    } else {
                                        name.clone()
                                    };
                                    egui::CollapsingHeader::new(&display_name)
                                        .id_salt(format!("mat_{}", i))
                                        .default_open(false)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let swatch = egui::color_picker::show_color(
                                                    ui,
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        (color[0] * 255.0) as u8,
                                                        (color[1] * 255.0) as u8,
                                                        (color[2] * 255.0) as u8,
                                                        (color[3] * 255.0) as u8,
                                                    ),
                                                    egui::vec2(16.0, 16.0),
                                                );
                                                let _ = swatch;
                                                ui.label(format!(
                                                    "RGBA: ({:.2}, {:.2}, {:.2}, {:.2})",
                                                    color[0], color[1], color[2], color[3]
                                                ));
                                            });
                                            ui.label(format!("Metallic: {:.2}", metallic));
                                            ui.label(format!("Roughness: {:.2}", roughness));
                                        });
                                }
                            });

                        ui.add_space(4.0);
                    }

                    // ── View Controls ──
                    egui::CollapsingHeader::new("View")
                        .default_open(true)
                        .show(ui, |ui| {
                            // Debug mode combo
                            let modes = [
                                DebugMode::Pbr,
                                DebugMode::WireframeOnly,
                                DebugMode::Normals,
                                DebugMode::Depth,
                                DebugMode::UvChecker,
                                DebugMode::Unlit,
                                DebugMode::MetallicRoughness,
                            ];
                            let mut selected = current_debug_mode;
                            egui::ComboBox::from_label("Shading")
                                .selected_text(selected.label())
                                .show_ui(ui, |ui| {
                                    for &mode in &modes {
                                        ui.selectable_value(&mut selected, mode, mode.label());
                                    }
                                });
                            if selected != current_debug_mode {
                                new_debug_mode = Some(selected);
                            }

                            // Wireframe overlay
                            let mut wf = current_wireframe;
                            ui.checkbox(&mut wf, "Wireframe overlay");
                            if wf != current_wireframe {
                                new_wireframe = Some(wf);
                            }

                            // Normal arrows
                            let mut na = current_normals;
                            ui.checkbox(&mut na, "Show normals");
                            if na != current_normals {
                                new_normals = Some(na);
                            }

                            // Grid
                            let mut gr = current_grid;
                            ui.checkbox(&mut gr, "Show grid");
                            if gr != current_grid {
                                new_grid = Some(gr);
                            }
                        });
                });

            // ── Bottom panel: animation timeline ──
            if has_anim {
                egui::TopBottomPanel::bottom("animation_panel")
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.add_space(4.0);

                        // Top row: play/pause, clip selector, speed
                        ui.horizontal(|ui| {
                            // Play/pause button
                            let btn_text = if anim_paused { "\u{25b6}" } else { "\u{23f8}" };
                            if ui.button(btn_text).clicked() {
                                new_anim_paused = Some(!anim_paused);
                            }

                            // Clip selector
                            if anim_clip_names.len() > 1 {
                                let current_name = &anim_clip_names[anim_clip_index];
                                let mut sel_idx = anim_clip_index;
                                egui::ComboBox::from_id_salt("clip_selector")
                                    .selected_text(current_name)
                                    .show_ui(ui, |ui| {
                                        for (i, name) in anim_clip_names.iter().enumerate() {
                                            ui.selectable_value(&mut sel_idx, i, name);
                                        }
                                    });
                                if sel_idx != anim_clip_index {
                                    new_clip_index = Some(sel_idx);
                                }
                            } else if !anim_clip_names.is_empty() {
                                ui.label(&anim_clip_names[0]);
                            }

                            ui.separator();

                            // Speed control
                            let mut spd = anim_speed;
                            ui.label("Speed:");
                            let drag = egui::DragValue::new(&mut spd)
                                .range(0.1..=10.0)
                                .speed(0.05)
                                .suffix("x");
                            if ui.add(drag).changed() {
                                new_speed = Some(spd);
                            }
                            if ui.small_button("1x").clicked() {
                                new_speed = Some(1.0);
                            }
                        });

                        // Timeline slider
                        if anim_duration > 0.0 {
                            ui.horizontal(|ui| {
                                ui.monospace(format!("{:.2}s", anim_time));

                                let mut t = anim_time;
                                let slider = egui::Slider::new(&mut t, 0.0..=anim_duration)
                                    .show_value(false)
                                    .trailing_fill(true);
                                let response = ui.add(slider);
                                if response.dragged() || response.changed() {
                                    scrub_time = Some(t);
                                    if !anim_paused {
                                        new_anim_paused = Some(true);
                                    }
                                }

                                ui.monospace(format!("{:.2}s", anim_duration));
                            });

                            // Frame counter
                            let frame = (anim_time * 30.0) as u32;
                            let total_frames = (anim_duration * 30.0) as u32;
                            ui.label(format!("Frame: {} / {}", frame, total_frames));
                        }

                        ui.add_space(2.0);
                    });
            }

            // ── Top-right overlay: FPS, camera info ──
            egui::Area::new(egui::Id::new("preview_overlay"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_black_alpha(160))
                        .rounding(4.0)
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(180, 220, 180),
                                format!("FPS: {:.0}", fps),
                            );
                            ui.label(format!("Mode: {}", current_debug_mode.label()));
                            ui.label(format!(
                                "Cam: d={:.1} y={:.0}\u{b0} p={:.0}\u{b0}",
                                cam_distance, cam_yaw_deg, cam_pitch_deg
                            ));
                        });
                });
        });

        // Apply mutations
        egui_winit.handle_platform_output(&window, full_output.platform_output);

        // Tessellate and render
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

        // Apply collected mutations to renderer/animation state
        if let Some(mode) = new_debug_mode {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.set_debug_mode(mode);
            }
        }
        if let Some(wf) = new_wireframe {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.debug_state_mut().wireframe_overlay = wf;
            }
        }
        if let Some(na) = new_normals {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.debug_state_mut().show_normals = na;
            }
        }
        if let Some(gr) = new_grid {
            if let (Some(renderer), Some(ctx)) =
                (&mut self.scene_renderer, &self.render_context)
            {
                renderer.set_show_grid(&ctx.device, gr);
            }
        }
        if let Some(paused) = new_anim_paused {
            self.anim_paused = paused;
            if let Ok(mut state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    if let Some(components) = state.world.get_components_mut(eid) {
                        components.set_field(
                            "animator",
                            "playing",
                            toml::Value::Boolean(!paused),
                        );
                    }
                }
            }
        }
        if let Some(idx) = new_clip_index {
            self.switch_clip(idx);
        }
        if let Some(spd) = new_speed {
            if let Ok(mut state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    if let Some(components) = state.world.get_components_mut(eid) {
                        components.set_field(
                            "animator",
                            "speed",
                            toml::Value::Float(spd),
                        );
                    }
                }
            }
        }
        if let Some(t) = scrub_time {
            if let Ok(state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    self.animation.skeletal_sync.set_playback_time(&eid, t);
                    self.animation.node_sync.set_playback_time(&eid, t);
                    // Advance with dt=0 to recompute bone matrices at the new time
                    self.animation.skeletal_sync.advance_and_compute(0.0);
                }
            }
            // Upload bone matrices after scrub
            if let (Some(renderer), Some(ctx)) =
                (&mut self.scene_renderer, &self.render_context)
            {
                for (entity_id, asset_name) in &self.skeletal_entity_assets {
                    if let Some(matrices) =
                        self.animation.skeletal_sync.bone_matrices(entity_id)
                    {
                        renderer.update_bone_matrices(&ctx.queue, asset_name, matrices);
                    }
                }
            }
        }
    }
}

/// Recursively render a node tree in the UI
fn render_node_tree(
    ui: &mut egui::Ui,
    nodes: &[(String, Vec<usize>, bool)],
    node_idx: usize,
    depth: usize,
) {
    if node_idx >= nodes.len() {
        return;
    }
    let (name, children, has_mesh) = &nodes[node_idx];
    let display_name = if name.is_empty() {
        format!("Node {}", node_idx)
    } else {
        name.clone()
    };
    let icon = if *has_mesh { "\u{25a0} " } else { "\u{25cb} " };
    let label = format!("{}{}", icon, display_name);

    if children.is_empty() {
        ui.indent(format!("node_{}", node_idx), |ui| {
            ui.label(label);
        });
    } else {
        egui::CollapsingHeader::new(label)
            .id_salt(format!("node_{}", node_idx))
            .default_open(depth < 2)
            .show(ui, |ui| {
                for &child_idx in children {
                    render_node_tree(ui, nodes, child_idx, depth + 1);
                }
            });
    }
}

impl ApplicationHandler for PreviewApp {
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
        // Forward events to egui first
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
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::Tab) => {
                            self.show_ui = !self.show_ui;
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            // Reset camera to auto-fit
                            if let Ok(state) = self.state.lock() {
                                if let Some(model_path) = &state.model_path {
                                    if let Ok(r) = import_gltf(model_path) {
                                        if let Some(b) = r.bounds() {
                                            auto_fit_camera(&b, &mut self.camera);
                                            self.camera.update_orbit();
                                        }
                                    }
                                }
                            }
                        }
                        // Animation controls
                        PhysicalKey::Code(KeyCode::KeyP) => {
                            if self.anim_info.is_some() {
                                self.anim_paused = !self.anim_paused;
                                // Update playing state in ECS
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            components.set_field(
                                                "animator",
                                                "playing",
                                                toml::Value::Boolean(!self.anim_paused),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            // Next clip
                            if let Some(info) = &self.anim_info {
                                let next = (info.current_clip_index + 1) % info.clip_names.len();
                                self.switch_clip(next);
                            }
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) => {
                            // Previous clip
                            if let Some(info) = &self.anim_info {
                                let prev = if info.current_clip_index == 0 {
                                    info.clip_names.len() - 1
                                } else {
                                    info.current_clip_index - 1
                                };
                                self.switch_clip(prev);
                            }
                        }
                        PhysicalKey::Code(KeyCode::Equal) => {
                            // Increase speed (×1.5)
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            let current = components
                                                .get("animator")
                                                .and_then(|a| {
                                                    a.get("speed").and_then(|v| {
                                                        v.as_float().or_else(|| {
                                                            v.as_integer().map(|i| i as f64)
                                                        })
                                                    })
                                                })
                                                .unwrap_or(1.0);
                                            let new_speed = (current * 1.5).min(10.0);
                                            components.set_field(
                                                "animator",
                                                "speed",
                                                toml::Value::Float(new_speed),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::Minus) => {
                            // Decrease speed (÷1.5)
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            let current = components
                                                .get("animator")
                                                .and_then(|a| {
                                                    a.get("speed").and_then(|v| {
                                                        v.as_float().or_else(|| {
                                                            v.as_integer().map(|i| i as f64)
                                                        })
                                                    })
                                                })
                                                .unwrap_or(1.0);
                                            let new_speed = (current / 1.5).max(0.1);
                                            components.set_field(
                                                "animator",
                                                "speed",
                                                toml::Value::Float(new_speed),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::Digit0) => {
                            // Reset speed to 1.0
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            components.set_field(
                                                "animator",
                                                "speed",
                                                toml::Value::Float(1.0),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    if let Some(context) = &mut self.render_context {
                        context.resize(new_size);
                        self.camera.aspect = context.aspect_ratio();
                        if let Some(renderer) = &mut self.scene_renderer {
                            renderer.resize_postprocess(
                                &context.device,
                                new_size.width,
                                new_size.height,
                            );
                        }
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.load_model_file(path);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.egui_ctx.is_pointer_over_area() {
                    // egui is handling this click
                } else {
                    match button {
                        MouseButton::Left => {
                            self.mouse_pressed = state == ElementState::Pressed;
                            if !self.mouse_pressed {
                                self.last_mouse_pos = None;
                            }
                        }
                        MouseButton::Right => {
                            self.right_mouse_pressed = state == ElementState::Pressed;
                            if !self.right_mouse_pressed {
                                self.last_mouse_pos = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x, position.y);
                if !self.egui_ctx.is_pointer_over_area() {
                    if let Some((lx, ly)) = self.last_mouse_pos {
                        let dx = (x - lx) as f32;
                        let dy = (y - ly) as f32;

                        if self.mouse_pressed {
                            // Orbit
                            self.camera.yaw -= dx * 0.005;
                            self.camera.pitch += dy * 0.005;
                            self.camera.pitch = self.camera.pitch.clamp(-1.4, 1.4);
                            self.camera.update_orbit();
                        } else if self.right_mouse_pressed {
                            // Pan
                            let right_x = self.camera.yaw.cos();
                            let right_z = -self.camera.yaw.sin();
                            let up_y = 1.0;

                            let pan_speed = self.camera.distance * 0.002;
                            self.camera.target.x -= dx * right_x * pan_speed;
                            self.camera.target.z -= dx * right_z * pan_speed;
                            self.camera.target.y += dy * up_y * pan_speed;
                            self.camera.update_orbit();
                        }
                    }
                }
                self.last_mouse_pos = Some((x, y));
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
                let dt = self.last_frame_time.elapsed().as_secs_f64();
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

                // Check for file-watch reload
                self.check_reload();

                // Update window title with animation status
                if self.anim_info.is_some() {
                    self.update_title();
                }

                let context = match &self.render_context {
                    Some(c) => c,
                    None => return,
                };
                let renderer = match &mut self.scene_renderer {
                    Some(r) => r,
                    None => return,
                };

                // Animation update
                if self.anim_info.is_some() && !self.anim_paused {
                    if let Ok(mut state) = self.state.lock() {
                        // Sync from world picks up component changes (clip switches, speed, etc.)
                        self.animation
                            .sync
                            .sync_from_world(&state.world, &self.animation.player);
                        self.animation.skeletal_sync.sync_from_world(&state.world);
                        self.animation.node_sync.sync_from_world(&state.world);

                        // Advance all animation tiers
                        self.animation
                            .sync
                            .advance_and_write(&mut state.world, &self.animation.player, dt);
                        self.animation.skeletal_sync.advance_and_compute(dt);
                        self.animation
                            .node_sync
                            .advance_and_apply(&mut state.world, dt);

                        self.anim_time_accumulator += dt;
                    }

                    // Upload bone matrices for skinned meshes
                    for (entity_id, asset_name) in &self.skeletal_entity_assets {
                        if let Some(matrices) =
                            self.animation.skeletal_sync.bone_matrices(entity_id)
                        {
                            renderer
                                .update_bone_matrices(&context.queue, asset_name, matrices);
                        }
                    }
                }

                if let Ok(state) = self.state.lock() {
                    renderer.update_from_world(&state.world, &context.device);
                }

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
                        eprintln!("Out of GPU memory");
                        event_loop.exit();
                        return;
                    }
                    Err(e) => {
                        eprintln!("Surface error: {:?}", e);
                        return;
                    }
                };

                let view = output
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                let _ = renderer.render(context, &self.camera, &view);

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
