//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{GameClock, InputState, RuntimeSystem};
use std::path::Path;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

pub struct PlayerApp {
    // Core state
    pub world: FlintWorld,
    pub scene_path: String,

    // Systems
    pub clock: GameClock,
    pub input: InputState,
    pub physics: PhysicsSystem,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Window options
    pub fullscreen: bool,
    cursor_captured: bool,
}

impl PlayerApp {
    pub fn new(world: FlintWorld, scene_path: String, fullscreen: bool) -> Self {
        Self {
            world,
            scene_path,
            clock: GameClock::new(),
            input: InputState::new(),
            physics: PhysicsSystem::new(),
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            fullscreen,
            cursor_captured: false,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let window_attrs = Window::default_attributes()
            .with_title("Flint Player")
            .with_inner_size(PhysicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());

        if self.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        self.window = Some(window.clone());

        // Initialize rendering
        let render_context = pollster::block_on(RenderContext::new(window.clone())).unwrap();

        self.camera.aspect = render_context.aspect_ratio();
        self.camera.fov = 70.0; // Slightly wider FOV for first-person

        let mut scene_renderer = SceneRenderer::new(&render_context);

        // Load models from world
        load_models_from_world(
            &self.world,
            &mut scene_renderer,
            &render_context.device,
            &render_context.queue,
            &self.scene_path,
        );
        scene_renderer.update_from_world(&self.world, &render_context.device);

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);

        // Initialize physics
        self.physics
            .initialize(&mut self.world)
            .expect("Failed to initialize physics");

        // Capture cursor for first-person look
        self.capture_cursor();
    }

    fn capture_cursor(&mut self) {
        if let Some(window) = &self.window {
            // Try confined first, then locked
            let _ = window.set_cursor_grab(CursorGrabMode::Confined)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked));
            window.set_cursor_visible(false);
            self.cursor_captured = true;
        }
    }

    fn release_cursor(&mut self) {
        if let Some(window) = &self.window {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.cursor_captured = false;
        }
    }

    fn render(&mut self) {
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };

        let output = match context.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
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

        if let Err(e) = renderer.render(context, &self.camera, &view) {
            eprintln!("Render error: {:?}", e);
        }

        output.present();
    }

    fn tick(&mut self) {
        // Advance game clock
        self.clock.tick();

        // Fixed-timestep physics loop
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            // Update character controller with current input
            self.physics.update_character(&self.input, &mut self.world, dt);

            // Step physics simulation
            self.physics
                .fixed_update(&mut self.world, dt)
                .unwrap_or_else(|e| eprintln!("Physics error: {:?}", e));

            self.clock.consume_fixed_step();
        }

        // Update camera from player character position
        let cam_pos = self.physics.character.camera_position(&self.world);
        let cam_target = self.physics.character.camera_target(cam_pos);
        self.camera.update_first_person(
            cam_pos,
            self.physics.character.yaw,
            self.physics.character.pitch,
        );
        // Also set target explicitly for view matrix
        self.camera.target = cam_target;

        // Clear per-frame input state
        self.input.end_frame();
    }
}

impl ApplicationHandler for PlayerApp {
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
                if let PhysicalKey::Code(key_code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            // Handle escape to toggle cursor capture
                            if key_code == KeyCode::Escape {
                                if self.cursor_captured {
                                    self.release_cursor();
                                } else {
                                    event_loop.exit();
                                }
                                return;
                            }

                            // Debug keys
                            match key_code {
                                KeyCode::F1 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let next = renderer.debug_state().mode.next();
                                        renderer.set_debug_mode(next);
                                        if let Some(context) = &self.render_context {
                                            renderer.update_from_world(
                                                &self.world,
                                                &context.device,
                                            );
                                        }
                                    }
                                }
                                KeyCode::F4 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        renderer.toggle_shadows();
                                    }
                                }
                                KeyCode::F11 => {
                                    if let Some(window) = &self.window {
                                        if window.fullscreen().is_some() {
                                            window.set_fullscreen(None);
                                        } else {
                                            window.set_fullscreen(Some(
                                                winit::window::Fullscreen::Borderless(None),
                                            ));
                                        }
                                    }
                                }
                                _ => {}
                            }

                            self.input.process_key_down(key_code);
                        }
                        ElementState::Released => {
                            self.input.process_key_up(key_code);
                        }
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if !self.cursor_captured {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.capture_cursor();
                    }
                    return;
                }

                let btn = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };

                match state {
                    ElementState::Pressed => self.input.process_mouse_button_down(btn),
                    ElementState::Released => self.input.process_mouse_button_up(btn),
                }
            }

            WindowEvent::RedrawRequested => {
                self.tick();
                self.render();
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.cursor_captured {
            return;
        }

        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.process_mouse_raw_delta(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Load glTF models referenced by entities in the world
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
            .and_then(|model| model.get("asset").and_then(|v| v.as_str().map(String::from)));

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                continue;
            }

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
            }
        }
    }

    // Also load textures
    load_textures_from_world(world, renderer, device, queue, scene_path);
}

/// Load texture files referenced by material components
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

    let mut loaded = std::collections::HashSet::new();

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
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("Failed to load texture '{}': {}", tex_name, e);
                    }
                }
            }
        }
    }
}
