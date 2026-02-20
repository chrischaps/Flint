//! Scene viewer with hot-reload

use anyhow::{Context, Result};
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_ecs::FlintWorld;
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{Camera, RenderContext, RendererConfig, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

pub fn run(scene_path: &str, watch: bool, schemas_path: &str) -> Result<()> {
    // Load schemas
    let registry = if Path::new(schemas_path).exists() {
        SchemaRegistry::load_from_directory(schemas_path).context("Failed to load schemas")?
    } else {
        println!("Warning: Schemas directory not found: {}", schemas_path);
        SchemaRegistry::new()
    };

    // Load initial scene
    let (world, scene_file) = load_scene(scene_path, &registry).context("Failed to load scene")?;

    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    // Create shared state
    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
    }));

    // Set up file watcher if requested
    let _watcher = if watch {
        let state_clone = Arc::clone(&state);
        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)
            .context("Failed to create file watcher")?;

        debouncer
            .watcher()
            .watch(Path::new(scene_path), RecursiveMode::NonRecursive)
            .context("Failed to watch scene file")?;

        // Spawn thread to handle file change events
        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_events) => {
                        if let Ok(mut state) = state_clone.lock() {
                            state.needs_reload = true;
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
    };

    // Run the event loop
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewerApp::new(state);
    event_loop.run_app(&mut app)?;

    Ok(())
}

struct ViewerState {
    world: FlintWorld,
    registry: SchemaRegistry,
    scene_path: String,
    needs_reload: bool,
}

struct ViewerApp {
    state: Arc<Mutex<ViewerState>>,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Animation
    animation: AnimationSystem,
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,
    last_frame_time: Option<Instant>,

    // Input state
    mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    right_mouse_pressed: bool,
}

impl ViewerApp {
    fn new(state: Arc<Mutex<ViewerState>>) -> Self {
        Self {
            state,
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            animation: AnimationSystem::new(),
            skeletal_entity_assets: HashMap::new(),
            last_frame_time: None,
            mouse_pressed: false,
            last_mouse_pos: None,
            right_mouse_pressed: false,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let window_attrs = Window::default_attributes()
            .with_title("Flint Scene Viewer")
            .with_inner_size(PhysicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());
        self.window = Some(window.clone());

        // Initialize rendering
        let render_context = pollster::block_on(RenderContext::new(window.clone())).unwrap();

        self.camera.aspect = render_context.aspect_ratio();
        self.camera.update_orbit();

        let mut scene_renderer = SceneRenderer::new(&render_context, RendererConfig { show_grid: true });

        // Load models (including skeletal data) and update meshes from world
        {
            let mut state = self.state.lock().unwrap();
            let config = ModelLoadConfig::from_scene_path(&state.scene_path);
            let load_result = model_loader::load_models_from_world(
                &mut state.world,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &config,
            );
            register_skeletal_data(&load_result, &mut self.animation);
            self.skeletal_entity_assets = load_result.skinned_entities;
            scene_renderer.update_from_world(&state.world, &render_context.device);

            // Load property animation clips from animations/ directory
            load_animations_from_world(&state.scene_path, &mut self.animation);

            // Initialize animation system (syncs entity states from world)
            self.animation
                .initialize(&mut state.world)
                .unwrap_or_else(|e| eprintln!("Animation init: {:?}", e));
        }

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);
    }

    fn render(&mut self) {
        let Some(context) = &self.render_context else { return };
        let Some(renderer) = &mut self.scene_renderer else { return };

        let output = match context.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                // Reconfigure surface
                return;
            }
            Err(e) => {
                eprintln!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        if let Err(e) = renderer.render(context, &self.camera, &view) {
            eprintln!("Render error: {:?}", e);
        }

        output.present();
    }

    fn tick_animation(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_frame_time
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.last_frame_time = Some(now);

        if dt <= 0.0 || dt > 0.5 {
            return; // Skip huge spikes (first frame, debugger pauses, etc.)
        }

        // Advance animations and write results to ECS
        {
            let mut state = self.state.lock().unwrap();
            self.animation.update(&mut state.world, dt).ok();
        }

        // Push skeletal bone matrices to GPU
        if let (Some(renderer), Some(context)) =
            (&mut self.scene_renderer, &self.render_context)
        {
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.animation.skeletal_sync.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(&context.queue, asset_name, matrices);
                }
            }
        }

        // Refresh renderer with updated transforms
        if let (Some(renderer), Some(context)) =
            (&mut self.scene_renderer, &self.render_context)
        {
            let state = self.state.lock().unwrap();
            renderer.update_from_world(&state.world, &context.device);
        }
    }

    fn check_reload(&mut self) {
        let (needs_reload, scene_path) = {
            let state = self.state.lock().unwrap();
            (state.needs_reload, state.scene_path.clone())
        };

        if needs_reload {
            // Mark as no longer needing reload first
            {
                let mut state = self.state.lock().unwrap();
                state.needs_reload = false;
            }

            // Load scene fresh from file
            let reload_result = {
                let state = self.state.lock().unwrap();
                load_scene(&scene_path, &state.registry)
            };

            match reload_result {
                Ok((new_world, scene_file)) => {
                    println!("Reloaded scene: {}", scene_file.scene.name);

                    // Update world
                    {
                        let mut state = self.state.lock().unwrap();
                        state.world = new_world;
                    }

                    // Reset animation system for fresh reload
                    self.animation = AnimationSystem::new();

                    // Reload models (including skeletal) and update renderer
                    if let (Some(context), Some(renderer)) =
                        (&self.render_context, &mut self.scene_renderer)
                    {
                        let mut state = self.state.lock().unwrap();
                        let config = ModelLoadConfig::from_scene_path(&state.scene_path);
                        let load_result = model_loader::load_models_from_world(
                            &mut state.world,
                            renderer,
                            &context.device,
                            &context.queue,
                            &config,
                        );
                        register_skeletal_data(&load_result, &mut self.animation);
                        self.skeletal_entity_assets = load_result.skinned_entities;
                        renderer.update_from_world(&state.world, &context.device);

                        // Reload animation clips and re-initialize
                        load_animations_from_world(&state.scene_path, &mut self.animation);
                        self.animation
                            .initialize(&mut state.world)
                            .unwrap_or_else(|e| eprintln!("Animation re-init: {:?}", e));
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reload scene: {:?}", e);
                }
            }
        }
    }
}

/// Register skeletal data from loaded models into the animation system.
fn register_skeletal_data(
    load_result: &model_loader::ModelLoadResult,
    animation: &mut AnimationSystem,
) {
    for loaded in &load_result.models {
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
                        clip.name,
                        clip.duration,
                        clip.joint_tracks.len()
                    );
                    animation.skeletal_sync.add_clip(clip);
                }
            }
        }
    }
}

/// Load `.anim.toml` files from the `animations/` directory next to the scene
fn load_animations_from_world(scene_path: &str, animation: &mut AnimationSystem) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let anim_dir = scene_dir.join("animations");
    if !anim_dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(&anim_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.ends_with(".anim.toml"))
        {
            match flint_animation::loader::load_clip_from_file(&path) {
                Ok(clip) => {
                    println!("Loaded animation: {} ({:.1}s)", clip.name, clip.duration);
                    animation.player.add_clip(clip);
                }
                Err(e) => {
                    eprintln!("Failed to load animation '{}': {:?}", path.display(), e);
                }
            }
        }
    }
}


impl ApplicationHandler for ViewerApp {
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
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
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

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            // Manual reload
                            if let Ok(mut state) = self.state.lock() {
                                state.needs_reload = true;
                            }
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            // Reset camera
                            self.camera = Camera::new();
                            self.camera.update_orbit();
                            if let Some(context) = &self.render_context {
                                self.camera.aspect = context.aspect_ratio();
                            }
                        }
                        PhysicalKey::Code(KeyCode::F1) => {
                            // Cycle debug shading mode
                            if let Some(renderer) = &mut self.scene_renderer {
                                let next = renderer.debug_state().mode.next();
                                renderer.set_debug_mode(next);
                                println!("Debug mode: {}", next.label());

                                // Regenerate auxiliary draws when entering/leaving wireframe mode
                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F2) => {
                            // Toggle wireframe overlay
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_wireframe_overlay();
                                println!("Wireframe overlay: {}", if on { "ON" } else { "OFF" });

                                // Regenerate auxiliary draws
                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F3) => {
                            // Toggle normal direction arrows
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_normal_arrows();
                                println!("Normal arrows: {}", if on { "ON" } else { "OFF" });

                                // Regenerate auxiliary draws
                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F4) => {
                            // Toggle shadow mapping
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_shadows();
                                println!("Shadows: {}", if on { "ON" } else { "OFF" });
                            }
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                match button {
                    MouseButton::Left => {
                        self.mouse_pressed = state == ElementState::Pressed;
                    }
                    MouseButton::Right => {
                        self.right_mouse_pressed = state == ElementState::Pressed;
                    }
                    _ => {}
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if let Some((last_x, last_y)) = self.last_mouse_pos {
                    let dx = (position.x - last_x) as f32;
                    let dy = (position.y - last_y) as f32;

                    if self.mouse_pressed {
                        // Orbit camera
                        self.camera.orbit_horizontal(-dx * 0.01);
                        self.camera.orbit_vertical(-dy * 0.01);
                    }

                    if self.right_mouse_pressed {
                        // Pan camera
                        self.camera.pan(-dx * 0.02, dy * 0.02);
                    }
                }

                self.last_mouse_pos = Some((position.x, position.y));
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 100.0,
                };
                self.camera.zoom(scroll);
            }

            WindowEvent::RedrawRequested => {
                self.tick_animation();
                self.check_reload();
                self.render();
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
