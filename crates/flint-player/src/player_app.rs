//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_audio::AudioSystem;
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{GameClock, InputState, RuntimeSystem};
use std::collections::HashMap;
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
    pub audio: AudioSystem,
    pub animation: AnimationSystem,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

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
            audio: AudioSystem::new(),
            animation: AnimationSystem::new(),
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            skeletal_entity_assets: HashMap::new(),
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

        // Load models from world (including skeletal data)
        self.skeletal_entity_assets = load_models_from_world(
            &self.world,
            &mut scene_renderer,
            &mut self.animation,
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

        // Initialize audio
        load_audio_from_world(&self.world, &mut self.audio, &self.scene_path);
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Audio init: {:?}", e));

        // Initialize animation
        load_animations_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Animation init: {:?}", e));

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

        // Update audio listener to match camera
        self.audio.update_listener(
            cam_pos,
            self.physics.character.yaw,
            self.physics.character.pitch,
        );

        // Process physics events for audio triggers
        let physics_events = self.physics.event_bus.drain();
        self.audio.process_events(&physics_events, &self.world);
        self.audio
            .update(&mut self.world, self.clock.delta_time)
            .ok();

        // Advance animations and write results to ECS transforms
        self.animation
            .update(&mut self.world, self.clock.delta_time)
            .ok();

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

        // Refresh renderer with updated transforms (animation may have changed positions)
        if let (Some(renderer), Some(context)) =
            (&mut self.scene_renderer, &self.render_context)
        {
            renderer.update_from_world(&self.world, &context.device);
        }

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

/// Load glTF models referenced by entities in the world.
/// Returns a mapping of entity_id → asset_name for skinned entities.
fn load_models_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    animation: &mut AnimationSystem,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) -> HashMap<flint_core::EntityId, String> {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut skeletal_entity_assets = HashMap::new();

    for entity in world.all_entities() {
        let model_asset = world
            .get_components(entity.id)
            .and_then(|components| components.get("model").cloned())
            .and_then(|model| model.get("asset").and_then(|v| v.as_str().map(String::from)));

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                // Even if already loaded, check if this entity needs skeletal tracking
                if renderer.mesh_cache().contains_skinned(&asset_name) {
                    if !animation.skeletal_sync.has_skeleton(&entity.id) {
                        // Re-import to get skeleton data (only on first encounter)
                        // The mesh is already cached, but we need the skeleton
                    }
                    skeletal_entity_assets.insert(entity.id, asset_name.clone());
                }
                continue;
            }

            let model_path = scene_dir.join("models").join(format!("{}.glb", asset_name));

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        let has_skins = !import_result.skeletons.is_empty();
                        let has_skinned_meshes = import_result
                            .meshes
                            .iter()
                            .any(|m| m.joint_indices.is_some());

                        println!(
                            "Loaded model: {} ({} meshes, {} materials{})",
                            asset_name,
                            import_result.meshes.len(),
                            import_result.materials.len(),
                            if has_skins {
                                format!(
                                    ", {} skins, {} skeletal clips",
                                    import_result.skeletons.len(),
                                    import_result.skeletal_clips.len()
                                )
                            } else {
                                String::new()
                            }
                        );

                        if has_skinned_meshes && has_skins {
                            // Upload as skinned mesh
                            renderer.load_skinned_model(
                                device,
                                queue,
                                &asset_name,
                                &import_result,
                            );
                            // Also upload static meshes (unskinned parts of the model)
                            renderer.load_model(device, queue, &asset_name, &import_result);

                            // Register skeletons with the animation system
                            for imported_skel in &import_result.skeletons {
                                let skeleton = Skeleton::from_imported(imported_skel);
                                animation
                                    .skeletal_sync
                                    .add_skeleton(entity.id, skeleton);
                            }

                            // Register skeletal clips
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

                            skeletal_entity_assets.insert(entity.id, asset_name.clone());
                        } else {
                            // Standard static mesh upload
                            renderer.load_model(device, queue, &asset_name, &import_result);
                        }
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

    skeletal_entity_assets
}

/// Load audio files referenced by audio_source components
fn load_audio_from_world(world: &FlintWorld, audio: &mut AudioSystem, scene_path: &str) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let audio_file = world
            .get_components(entity.id)
            .and_then(|components| components.get("audio_source").cloned())
            .and_then(|audio_src| {
                audio_src
                    .get("file")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(file_name) = audio_file {
            if audio.engine.has_sound(&file_name) {
                continue;
            }

            let audio_path = scene_dir.join(&file_name);
            if audio_path.exists() {
                match audio.engine.load_sound(&file_name, &audio_path) {
                    Ok(_) => {
                        println!("Loaded audio: {}", file_name);
                    }
                    Err(e) => {
                        eprintln!("Failed to load audio '{}': {:?}", file_name, e);
                    }
                }
            } else {
                eprintln!("Audio file not found: {}", audio_path.display());
            }
        }
    }
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
