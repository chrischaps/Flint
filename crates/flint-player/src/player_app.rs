//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

use anyhow::{Context, Result};
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_core::Vec3 as FlintVec3;
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{
    Binding, GameClock, GameEvent, InputConfig, InputState, RebindMode, RuntimeSystem,
};
use flint_script::context::{DrawCommand, LogLevel, ScriptCommand};
use flint_script::ScriptSystem;
use gilrs::{Axis, Button, EventType, Gilrs};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

#[derive(Debug, Clone)]
struct PendingRebind {
    action: String,
    mode: RebindMode,
}

#[derive(Debug, Clone)]
struct InputConfigPaths {
    game_default: Option<PathBuf>,
    user_override: Option<PathBuf>,
    cli_override: Option<PathBuf>,
}

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
    pub script: ScriptSystem,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

    // HUD + egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,

    // Script-driven 2D draw commands
    draw_commands: Vec<DrawCommand>,
    ui_textures: HashMap<String, egui::TextureHandle>,

    // Asset catalog (optional, for content-addressed asset resolution)
    catalog: Option<AssetCatalog>,
    content_store: Option<ContentStore>,

    // Window options
    pub fullscreen: bool,
    cursor_captured: bool,

    // Input config layering + remap persistence
    input_config_override: Option<String>,
    scene_input_config: Option<String>,
    input_config_paths: Option<InputConfigPaths>,
    user_override_config: InputConfig,
    pending_rebind: Option<PendingRebind>,

    // Optional gamepad backend
    gilrs: Option<Gilrs>,
}

impl PlayerApp {
    pub fn new(
        world: FlintWorld,
        scene_path: String,
        fullscreen: bool,
        input_config_override: Option<String>,
        scene_input_config: Option<String>,
    ) -> Self {
        Self {
            world,
            scene_path,
            clock: GameClock::new(),
            input: InputState::new(),
            physics: PhysicsSystem::new(),
            audio: AudioSystem::new(),
            animation: AnimationSystem::new(),
            script: ScriptSystem::new(),
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            skeletal_entity_assets: HashMap::new(),
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            draw_commands: Vec::new(),
            ui_textures: HashMap::new(),
            catalog: AssetCatalog::load_from_directory("assets").ok(),
            content_store: Some(ContentStore::new(".flint/assets")),
            fullscreen,
            cursor_captured: false,
            input_config_override,
            scene_input_config,
            input_config_paths: None,
            user_override_config: InputConfig {
                version: 1,
                game_id: String::new(),
                actions: Default::default(),
            },
            pending_rebind: None,
            gilrs: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window_attrs = Window::default_attributes()
            .with_title("Flint Player")
            .with_inner_size(PhysicalSize::new(1280, 720));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create player window")?,
        );

        if self.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        self.window = Some(window.clone());

        // Initialize rendering
        let render_context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to initialize render context")?;

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
            self.catalog.as_ref(),
            self.content_store.as_ref(),
        );
        scene_renderer.update_from_world(&self.world, &render_context.device);

        // Load input configs with deterministic layering.
        self.configure_input_bindings()
            .unwrap_or_else(|e| eprintln!("Input config load error: {e:#}"));

        // Initialize gamepad backend (best-effort).
        self.gilrs = Gilrs::new().ok();

        // Initialize egui for HUD overlay
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &render_context.device,
            render_context.config.format,
            None,
            1,
            false,
        );
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);

        // Initialize physics
        self.physics
            .initialize(&mut self.world)
            .context("Failed to initialize physics")?;

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

        // Initialize scripting
        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Script init: {:?}", e));

        // Capture cursor for first-person look
        self.capture_cursor();

        Ok(())
    }

    /// Start capture mode for "press next control to bind" remapping.
    pub fn begin_rebind_capture(&mut self, action: impl Into<String>, mode: RebindMode) {
        self.pending_rebind = Some(PendingRebind {
            action: action.into(),
            mode,
        });
    }

    fn configure_input_bindings(&mut self) -> Result<()> {
        self.input
            .load_bindings(InputConfig::built_in_defaults())
            .context("failed to load built-in input defaults")?;

        let paths = resolve_input_paths(
            Path::new(&self.scene_path),
            self.scene_input_config.as_deref(),
            self.input_config_override.as_deref(),
        );

        if let Some(path) = &paths.game_default {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load game input config '{}'", path.display())
                })?;
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge game input config")?;
            }
        }

        if let Some(path) = &paths.user_override {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load user input config '{}'", path.display())
                })?;
                self.user_override_config = cfg.clone();
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge user input config")?;
            }
        }

        if let Some(path) = &paths.cli_override {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load CLI input config '{}'", path.display())
                })?;
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge CLI input config")?;
            }
        }

        if self.user_override_config.version == 0 {
            self.user_override_config.version = 1;
        }
        if self.user_override_config.game_id.trim().is_empty() {
            self.user_override_config.game_id = self.input.config().game_id.clone();
        }

        self.input_config_paths = Some(paths);
        Ok(())
    }

    fn poll_gamepad_events(&mut self) {
        let mut events = Vec::new();
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
        }

        for event in events {
            let gamepad = gamepad_id_to_u32(event.id);
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    let name = format!("{button:?}");
                    if self.try_capture_rebind(Binding::GamepadButton {
                        button: name.clone(),
                        gamepad: flint_runtime::GamepadSelector::Any,
                    }) {
                        continue;
                    }
                    self.input.process_gamepad_button_down(gamepad, name);
                }
                EventType::ButtonReleased(button, _) => {
                    let name = format!("{button:?}");
                    self.input.process_gamepad_button_up(gamepad, name);
                }
                EventType::AxisChanged(axis, value, _) => {
                    let name = format!("{axis:?}");
                    if self.pending_rebind.is_some() && value.abs() >= 0.45 {
                        let direction = if value < 0.0 {
                            Some(flint_runtime::AxisDirection::Negative)
                        } else {
                            Some(flint_runtime::AxisDirection::Positive)
                        };
                        if self.try_capture_rebind(Binding::GamepadAxis {
                            axis: name.clone(),
                            gamepad: flint_runtime::GamepadSelector::Any,
                            deadzone: 0.15,
                            scale: 1.0,
                            invert: false,
                            threshold: Some(0.35),
                            direction,
                        }) {
                            continue;
                        }
                    }
                    self.input.process_gamepad_axis(gamepad, name, value);
                }
                EventType::Disconnected => {
                    self.input.clear_gamepad(gamepad);
                }
                _ => {}
            }
        }
    }

    fn try_capture_rebind(&mut self, binding: Binding) -> bool {
        let Some(pending) = self.pending_rebind.take() else {
            return false;
        };

        if let Err(e) = self
            .input
            .rebind_action(&pending.action, binding, pending.mode)
        {
            eprintln!("Failed to rebind action '{}': {:?}", pending.action, e);
            return true;
        }

        if let Some(action_cfg) = self.input.action_config(&pending.action) {
            self.user_override_config
                .actions
                .insert(pending.action.clone(), action_cfg);
        }
        if self.user_override_config.game_id.trim().is_empty() {
            self.user_override_config.game_id = self.input.config().game_id.clone();
        }

        if let Err(e) = self.persist_user_overrides() {
            eprintln!("Failed to save input overrides: {e:#}");
        }

        true
    }

    fn persist_user_overrides(&mut self) -> Result<()> {
        let Some(paths) = &mut self.input_config_paths else {
            return Ok(());
        };

        let mut target = paths.user_override.clone().unwrap_or_else(|| {
            fallback_user_override_path(
                Path::new(&self.scene_path),
                &self.input.config().game_id,
            )
            .unwrap_or_else(|| PathBuf::from(".flint/input.user.toml"))
        });

        if let Err(err) = write_user_override_file(&target, &self.user_override_config) {
            let Some(fallback) =
                fallback_user_override_path(Path::new(&self.scene_path), &self.input.config().game_id)
            else {
                return Err(err);
            };
            if fallback != target {
                write_user_override_file(&fallback, &self.user_override_config)?;
                target = fallback;
            } else {
                return Err(err);
            }
        }

        paths.user_override = Some(target);
        Ok(())
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

        // Render egui HUD overlay on top of the 3D scene
        self.render_hud(&view);

        output.present();
    }

    fn tick(&mut self) {
        self.poll_gamepad_events();

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

        // Process physics events — scripts + audio both consume them
        let mut game_events = self.physics.event_bus.drain();

        // Generate ActionPressed events from input (drives on_action / on_interact callbacks)
        for action in self.input.actions_just_pressed() {
            game_events.push(flint_runtime::GameEvent::ActionPressed(action));
        }
        for action in self.input.actions_just_released() {
            game_events.push(flint_runtime::GameEvent::ActionReleased(action));
        }

        // Script system: provide physics + camera context, then run updates
        self.script.set_physics(&self.physics);
        self.script.set_camera(
            self.camera.position_array(),
            self.camera.forward_vector(),
        );
        self.script.provide_context(
            &self.input,
            &game_events,
            self.clock.total_time,
            self.clock.delta_time,
        );
        // Set screen size for UI draw functions (logical points, not physical pixels)
        let screen_rect = self.egui_ctx.screen_rect();
        self.script.set_screen_size(screen_rect.width(), screen_rect.height());
        self.script
            .update(&mut self.world, self.clock.delta_time)
            .unwrap_or_else(|e| eprintln!("Script error: {:?}", e));

        // Call on_draw_ui() for all scripts (generates draw commands)
        self.script.call_draw_uis(&mut self.world);

        let script_commands = self.script.drain_commands();
        self.process_script_commands(script_commands);

        // Collect draw commands for this frame
        self.draw_commands = self.script.drain_draw_commands();

        // Audio triggers from game events
        self.audio.process_events(&game_events, &self.world);
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

    fn render_hud(&mut self, target_view: &wgpu::TextureView) {
        // Lazy-load any sprite textures referenced by draw commands
        // (must happen before egui_winit borrow)
        self.load_pending_sprites();

        let Some(window) = &self.window else { return };
        let Some(context) = &self.render_context else { return };
        let Some(egui_winit) = &mut self.egui_winit else { return };

        let raw_input = egui_winit.take_egui_input(window);

        let draw_commands = std::mem::take(&mut self.draw_commands);
        let ui_textures = &self.ui_textures;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            render_draw_commands(ctx, &draw_commands, ui_textures);
        });

        self.draw_commands = draw_commands;

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder =
            context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("HUD Encoder"),
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
                label: Some("HUD Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // overlay on top of 3D
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
    }

    fn process_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            match cmd {
                ScriptCommand::PlaySound { name, volume } => {
                    if let Err(e) = self.audio.engine.play_non_spatial(&name, volume, 1.0, false) {
                        eprintln!("[script] play_sound error: {:?}", e);
                    }
                }
                ScriptCommand::PlaySoundAt { name, position, volume } => {
                    let pos = FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
                    if let Err(e) = self.audio.engine.play_at_position(&name, pos, volume) {
                        eprintln!("[script] play_sound_at error: {:?}", e);
                    }
                }
                ScriptCommand::StopSound { name: _ } => {
                    // One-shot sounds play to completion (same as AudioCommand::Stop)
                }
                ScriptCommand::FireEvent { name, data } => {
                    self.physics.event_bus.push(GameEvent::Custom { name, data });
                }
                ScriptCommand::Log { level, message } => {
                    match level {
                        LogLevel::Info => println!("[script] {}", message),
                        LogLevel::Warn => eprintln!("[script warn] {}", message),
                        LogLevel::Error => eprintln!("[script error] {}", message),
                    }
                }
            }
        }
    }
}

impl ApplicationHandler for PlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.initialize(event_loop) {
                eprintln!("Failed to initialize player: {e:#}");
                event_loop.exit();
            }
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
    catalog: Option<&AssetCatalog>,
    store: Option<&ContentStore>,
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

            // Try catalog lookup first (content-addressed storage)
            let catalog_path = catalog
                .and_then(|cat| cat.get(&asset_name))
                .and_then(|meta| {
                    let hash = flint_core::ContentHash::from_prefixed_hex(&meta.hash)?;
                    store.and_then(|s| s.get(&hash))
                });

            let model_path = if let Some(cp) = catalog_path {
                cp
            } else {
                // Search scene dir first, then parent (game root)
                let p = scene_dir.join("models").join(format!("{}.glb", asset_name));
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join("models").join(format!("{}.glb", asset_name))
                } else {
                    p
                }
            };

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        let has_skins = !import_result.skeletons.is_empty();
                        let has_skinned_meshes = import_result
                            .meshes
                            .iter()
                            .any(|m| m.joint_indices.is_some());

                        let bounds_info = import_result
                            .bounds()
                            .map(|b| format!(", bounds: {}", b))
                            .unwrap_or_default();

                        println!(
                            "Loaded model: {} ({} meshes, {} materials{}{})",
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
                            },
                            bounds_info,
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
    load_textures_from_world(world, renderer, device, queue, scene_path, catalog, store);

    skeletal_entity_assets
}

/// Load audio files referenced by audio_source components and preload all .ogg files from audio/
fn load_audio_from_world(world: &FlintWorld, audio: &mut AudioSystem, scene_path: &str) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    // Build list of directories to search: scene dir first, then parent (game root)
    let mut search_dirs = vec![scene_dir.to_path_buf()];
    if let Some(parent) = scene_dir.parent() {
        search_dirs.push(parent.to_path_buf());
    }

    // Load audio files referenced by audio_source components
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

            let mut loaded = false;
            for dir in &search_dirs {
                let audio_path = dir.join(&file_name);
                if audio_path.exists() {
                    match audio.engine.load_sound(&file_name, &audio_path) {
                        Ok(_) => {
                            println!("Loaded audio: {}", file_name);
                            loaded = true;
                        }
                        Err(e) => {
                            eprintln!("Failed to load audio '{}': {:?}", file_name, e);
                        }
                    }
                    break;
                }
            }
            if !loaded {
                eprintln!("Audio file not found: {}", file_name);
            }
        }
    }

    // Preload all audio files from the audio/ directory (for script-triggered sounds)
    for dir in &search_dirs {
        let audio_dir = dir.join("audio");
        if audio_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&audio_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "ogg" || ext == "wav" || ext == "mp3" || ext == "flac" {
                        let rel_name = format!("audio/{}", path.file_name().unwrap().to_string_lossy());
                        if audio.engine.has_sound(&rel_name) {
                            continue;
                        }
                        match audio.engine.load_sound(&rel_name, &path) {
                            Ok(_) => {
                                println!("Preloaded audio: {}", rel_name);
                            }
                            Err(e) => {
                                eprintln!("Failed to preload audio '{}': {:?}", rel_name, e);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Load texture files referenced by material and sprite components
fn load_textures_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
    catalog: Option<&AssetCatalog>,
    store: Option<&ContentStore>,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut loaded = std::collections::HashSet::new();

    for entity in world.all_entities() {
        let components = world.get_components(entity.id);

        // Collect texture names from material and sprite components
        let mut tex_names = Vec::new();

        if let Some(comps) = &components {
            if let Some(material) = comps.get("material") {
                if let Some(tex) = material.get("texture").and_then(|v| v.as_str()) {
                    tex_names.push(tex.to_string());
                }
            }
            if let Some(sprite) = comps.get("sprite") {
                if let Some(tex) = sprite.get("texture").and_then(|v| v.as_str()) {
                    if !tex.is_empty() {
                        tex_names.push(tex.to_string());
                    }
                }
            }
        }

        for tex_name in tex_names {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            // Try catalog lookup first
            let catalog_path = catalog
                .and_then(|cat| cat.get(&tex_name))
                .and_then(|meta| {
                    let hash = flint_core::ContentHash::from_prefixed_hex(&meta.hash)?;
                    store.and_then(|s| s.get(&hash))
                });

            let tex_path = if let Some(cp) = catalog_path {
                cp
            } else {
                // Search scene dir first, then parent (game root)
                let p = scene_dir.join(&tex_name);
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join(&tex_name)
                } else {
                    p
                }
            };
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

/// Load `.anim.toml` files from the `animations/` directory next to the scene.
/// Also checks one level up (game root) for projects that use a `scenes/` subdirectory.
fn load_animations_from_world(scene_path: &str, animation: &mut AnimationSystem) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut anim_dir = scene_dir.join("animations");
    if !anim_dir.is_dir() {
        // Check parent directory (game project structure)
        if let Some(parent) = scene_dir.parent() {
            let parent_anim = parent.join("animations");
            if parent_anim.is_dir() {
                anim_dir = parent_anim;
            } else {
                return;
            }
        } else {
            return;
        }
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

/// Set up the script system's scripts directory from the scene path
fn load_scripts_from_world(scene_path: &str, script: &mut ScriptSystem) {
    flint_script::sync::load_scripts_from_scene(scene_path, &mut script.sync);
}

// ─── Script-driven UI rendering ──────────────────────────

fn to_color32(c: &[f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

/// Render script-issued 2D draw commands via egui layer painter.
/// Uses `ctx.layer_painter()` directly instead of `egui::Area` to avoid
/// zero-size clipping when only painter calls are used (no widgets).
fn render_draw_commands(
    ctx: &egui::Context,
    commands: &[DrawCommand],
    ui_textures: &HashMap<String, egui::TextureHandle>,
) {
    if commands.is_empty() {
        return;
    }

    // Sort by layer (stable sort preserves insertion order within same layer)
    let mut sorted: Vec<&DrawCommand> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.layer());

    let layer_id = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("script_ui_overlay"),
    );
    let painter = ctx.layer_painter(layer_id);

    for cmd in &sorted {
        match cmd {
            DrawCommand::Text { x, y, text, size, color, .. } => {
                painter.text(
                    egui::Pos2::new(*x, *y),
                    egui::Align2::LEFT_TOP,
                    text,
                    egui::FontId::proportional(*size),
                    to_color32(color),
                );
            }

            DrawCommand::RectFilled { x, y, w, h, color, rounding, .. } => {
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(*x, *y),
                    egui::Vec2::new(*w, *h),
                );
                painter.rect_filled(rect, *rounding, to_color32(color));
            }

            DrawCommand::RectOutline { x, y, w, h, color, thickness, .. } => {
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(*x, *y),
                    egui::Vec2::new(*w, *h),
                );
                painter.rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::CircleFilled { x, y, radius, color, .. } => {
                painter.circle_filled(
                    egui::Pos2::new(*x, *y),
                    *radius,
                    to_color32(color),
                );
            }

            DrawCommand::CircleOutline { x, y, radius, color, thickness, .. } => {
                painter.circle_stroke(
                    egui::Pos2::new(*x, *y),
                    *radius,
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::Line { x1, y1, x2, y2, color, thickness, .. } => {
                painter.line_segment(
                    [egui::Pos2::new(*x1, *y1), egui::Pos2::new(*x2, *y2)],
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::Sprite { x, y, w, h, name, uv, tint, .. } => {
                if let Some(tex_handle) = ui_textures.get(name.as_str()) {
                    let rect = egui::Rect::from_min_size(
                        egui::Pos2::new(*x, *y),
                        egui::Vec2::new(*w, *h),
                    );
                    let uv_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(uv[0], uv[1]),
                        egui::Pos2::new(uv[2], uv[3]),
                    );
                    painter.image(tex_handle.id(), rect, uv_rect, to_color32(tint));
                }
            }
        }
    }
}

impl PlayerApp {
    /// Load a sprite texture for UI rendering. Called lazily when a draw_sprite
    /// command references a name not yet in ui_textures.
    pub fn load_ui_texture(&mut self, name: &str) -> bool {
        if self.ui_textures.contains_key(name) {
            return true;
        }

        let scene_dir = Path::new(&self.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."));

        // Search: scene_dir/sprites/{name} → game_root/sprites/{name} → scene_dir/{name}
        let candidates = [
            scene_dir.join("sprites").join(name),
            scene_dir.parent().map(|p| p.join("sprites").join(name)).unwrap_or_default(),
            scene_dir.join(name),
        ];

        for path in &candidates {
            if path.exists() {
                if let Ok(img) = image::open(path) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        &rgba,
                    );
                    let tex_handle = self.egui_ctx.load_texture(
                        name,
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.ui_textures.insert(name.to_string(), tex_handle);
                    println!("Loaded UI sprite: {}", name);
                    return true;
                }
            }
        }

        eprintln!("UI sprite not found: {}", name);
        false
    }

    /// Pre-scan draw commands and load any sprite textures that haven't been loaded yet
    fn load_pending_sprites(&mut self) {
        let sprite_names: Vec<String> = self.draw_commands.iter().filter_map(|cmd| {
            if let DrawCommand::Sprite { name, .. } = cmd {
                if !self.ui_textures.contains_key(name.as_str()) {
                    Some(name.clone())
                } else {
                    None
                }
            } else {
                None
            }
        }).collect();

        for name in sprite_names {
            self.load_ui_texture(&name);
        }
    }
}
