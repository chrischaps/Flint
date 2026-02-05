//! Scene viewer with hot-reload

use anyhow::{Context, Result};
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

        let mut scene_renderer = SceneRenderer::new(&render_context);

        // Load models and update meshes from world
        {
            let state = self.state.lock().unwrap();
            load_models_from_world(&state.world, &mut scene_renderer, &render_context.device, &render_context.queue, &state.scene_path);
            scene_renderer.update_from_world(&state.world, &render_context.device);
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

                    // Reload models and update renderer
                    if let (Some(context), Some(renderer)) =
                        (&self.render_context, &mut self.scene_renderer)
                    {
                        let state = self.state.lock().unwrap();
                        load_models_from_world(&state.world, renderer, &context.device, &context.queue, &state.scene_path);
                        renderer.update_from_world(&state.world, &context.device);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reload scene: {:?}", e);
                }
            }
        }
    }
}

/// Scan the world for entities with model components and load the referenced glTF files
fn load_models_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let model_asset = world
            .get_components(entity.id)
            .and_then(|components| components.get("model").cloned())
            .and_then(|model| {
                model
                    .get("asset")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                continue;
            }

            // Look for the model file in demo/models/ relative to the scene file
            let model_path = scene_dir.join("models").join(format!("{}.glb", asset_name));

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        println!(
                            "Loaded model: {} ({} meshes, {} materials)",
                            asset_name,
                            import_result.meshes.len(),
                            import_result.materials.len()
                        );
                        renderer.load_model(device, queue, &asset_name, &import_result);
                    }
                    Err(e) => {
                        eprintln!("Failed to load model '{}': {:?}", asset_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Model file not found: {} (tried {})",
                    asset_name,
                    model_path.display()
                );
            }
        }
    }

    // Also load texture files referenced by material components
    load_textures_from_world(world, renderer, device, queue, scene_path);
}

/// Scan the world for entities with material.texture and pre-load the referenced image files
fn load_textures_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut loaded: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entity in world.all_entities() {
        let texture_name = world
            .get_components(entity.id)
            .and_then(|components| components.get("material").cloned())
            .and_then(|material| {
                material
                    .get("texture")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(tex_name) = texture_name {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            let tex_path = scene_dir.join(&tex_name);
            if tex_path.exists() {
                match renderer.load_texture_file(device, queue, &tex_name, &tex_path) {
                    Ok(true) => {
                        println!("Loaded texture: {}", tex_name);
                    }
                    Ok(false) => {} // already cached
                    Err(e) => {
                        eprintln!("Failed to load texture '{}': {}", tex_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Texture file not found: {} (tried {})",
                    tex_name,
                    tex_path.display()
                );
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
