//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

use anyhow::{Context, Result};
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_core::{FlintError, Vec3 as FlintVec3};
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, ParticleDrawData, ParticleInstanceGpu, RenderContext, SceneRenderer};
use flint_runtime::{
    Binding, GameClock, GameEvent, GameStateMachine, InputConfig, InputState, PersistentStore,
    RebindMode, RuntimeSystem, SystemPolicy,
};
use flint_script::context::{DrawCommand, LogLevel, ScriptCommand};
use flint_script::ScriptSystem;
use gilrs::{EventType, Gilrs};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
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

/// Scene transition lifecycle phase
#[derive(Debug, Clone)]
enum TransitionPhase {
    /// Normal gameplay
    Idle,
    /// Playing exit transition — scripts draw fade-out visuals
    Exiting {
        target_scene: String,
        elapsed: f32,
    },
    /// Loading the new scene (synchronous, happens in one frame)
    Loading {
        target_scene: String,
    },
    /// Playing enter transition — scripts draw fade-in visuals
    Entering {
        elapsed: f32,
    },
}

impl TransitionPhase {
    fn is_idle(&self) -> bool {
        matches!(self, TransitionPhase::Idle)
    }
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
    pub particles: ParticleSystem,
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

    // Environment
    pub skybox_path: Option<String>,

    // Scene-level post-processing overrides
    pub scene_post_process: Option<flint_scene::PostProcessDef>,

    // Script-driven post-processing overrides (applied per-frame before render)
    pp_vignette_override: Option<f32>,
    pp_bloom_override: Option<f32>,
    pp_exposure_override: Option<f32>,
    pp_chromatic_aberration_override: Option<f32>,
    pp_radial_blur_override: Option<f32>,

    // Input config layering + remap persistence
    input_config_override: Option<String>,
    scene_input_config: Option<String>,
    input_config_paths: Option<InputConfigPaths>,
    user_override_config: InputConfig,
    pending_rebind: Option<PendingRebind>,

    // Optional gamepad backend
    gilrs: Option<Gilrs>,

    // State machine + persistence (survive scene transitions)
    state_machine: GameStateMachine,
    persistent_store: PersistentStore,

    // Scene transition lifecycle
    transition_phase: TransitionPhase,

    // Schema paths preserved across transitions
    schema_paths: Vec<String>,
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
            particles: ParticleSystem::new(),
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
            skybox_path: None,
            scene_post_process: None,
            pp_vignette_override: None,
            pp_bloom_override: None,
            pp_exposure_override: None,
            pp_chromatic_aberration_override: None,
            pp_radial_blur_override: None,
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
            state_machine: GameStateMachine::new(),
            persistent_store: PersistentStore::new(),
            transition_phase: TransitionPhase::Idle,
            schema_paths: Vec::new(),
        }
    }

    /// Set the schema paths used for scene loading (preserved across transitions).
    pub fn set_schema_paths(&mut self, paths: Vec<String>) {
        self.schema_paths = paths;
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

        let mut scene_renderer = SceneRenderer::new(&render_context, Default::default());

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

        // Generate procedural geometry from spline + spline_mesh entities
        crate::spline_gen::load_splines(
            &self.scene_path,
            &mut self.world,
            &mut scene_renderer,
            Some(&mut self.physics),
            &render_context.device,
        );

        // Refresh renderer with any new procedural meshes
        scene_renderer.update_from_world(&self.world, &render_context.device);

        // Load skybox if configured
        if let Some(skybox_rel) = &self.skybox_path {
            let scene_dir = Path::new(&self.scene_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));

            // Search scene dir first, then parent (game root)
            let skybox_path = {
                let p = scene_dir.join(skybox_rel);
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join(skybox_rel)
                } else {
                    p
                }
            };

            if skybox_path.exists() {
                scene_renderer.load_skybox(
                    &render_context.device,
                    &render_context.queue,
                    &skybox_path,
                );
            } else {
                eprintln!("Skybox file not found: {}", skybox_path.display());
            }
        }

        // Apply scene-level post-processing config
        if let Some(pp_def) = &self.scene_post_process {
            use flint_render::PostProcessConfig;
            let mut config = PostProcessConfig::default();
            config.bloom_enabled = pp_def.bloom_enabled;
            config.bloom_intensity = pp_def.bloom_intensity;
            config.bloom_threshold = pp_def.bloom_threshold;
            config.vignette_enabled = pp_def.vignette_enabled;
            config.vignette_intensity = pp_def.vignette_intensity;
            config.vignette_smoothness = pp_def.vignette_smoothness;
            config.exposure = pp_def.exposure;
            scene_renderer.set_post_process_config(config);
        }

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

        // Initialize particles
        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Particles init: {:?}", e));

        // Initialize scripting
        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script.set_current_scene(&self.scene_path);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Script init: {:?}", e));

        // Capture cursor for first-person look (only if FPS player exists)
        if self.physics.character.player_entity().is_some() {
            self.capture_cursor();
        }

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
                EventType::ButtonChanged(button, value, _) => {
                    let name = format!("{button:?}");
                    self.input.process_gamepad_button_changed(gamepad, name, value);
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

        // Apply script-driven post-processing overrides before rendering
        if self.pp_vignette_override.is_some()
            || self.pp_bloom_override.is_some()
            || self.pp_exposure_override.is_some()
            || self.pp_chromatic_aberration_override.is_some()
            || self.pp_radial_blur_override.is_some()
        {
            let mut config = renderer.post_process_config().clone();
            if let Some(v) = self.pp_vignette_override {
                config.vignette_enabled = v > 0.001;
                config.vignette_intensity = v;
            }
            if let Some(b) = self.pp_bloom_override {
                config.bloom_intensity = b;
            }
            if let Some(e) = self.pp_exposure_override {
                config.exposure = e;
            }
            if let Some(ca) = self.pp_chromatic_aberration_override {
                config.chromatic_aberration = ca;
            }
            if let Some(rb) = self.pp_radial_blur_override {
                config.radial_blur = rb;
            }
            renderer.set_post_process_config(config);
        }

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

        // Advance transition phase timing
        self.advance_transition();

        // Read active state config to decide which systems run
        let config = self.state_machine.active_config().clone();

        let has_fps_player = self.physics.character.player_entity().is_some();

        // Fixed-timestep physics loop (skip when paused, but still consume steps to avoid spiral)
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            if config.physics == SystemPolicy::Run {
                if has_fps_player {
                    self.physics.update_character(&self.input, &mut self.world, dt);
                }
                self.physics
                    .fixed_update(&mut self.world, dt)
                    .unwrap_or_else(|e| eprintln!("Physics error: {:?}", e));
            }

            self.clock.consume_fixed_step();
        }

        // Update camera from player character position (FPS mode)
        if has_fps_player {
            let cam_pos = self.physics.character.camera_position(&self.world);
            let cam_target = self.physics.character.camera_target(cam_pos);
            self.camera.update_first_person(
                cam_pos,
                self.physics.character.yaw,
                self.physics.character.pitch,
            );
            self.camera.target = cam_target;

            if config.audio == SystemPolicy::Run {
                self.audio.update_listener(
                    cam_pos,
                    self.physics.character.yaw,
                    self.physics.character.pitch,
                );
            }
        }

        // Process physics events — scripts + audio both consume them
        // Always collect events (input always processed so pause/unpause keybinds work)
        let mut game_events = self.physics.event_bus.drain();
        for action in self.input.actions_just_pressed() {
            game_events.push(flint_runtime::GameEvent::ActionPressed(action));
        }
        for action in self.input.actions_just_released() {
            game_events.push(flint_runtime::GameEvent::ActionReleased(action));
        }

        // Set state machine + persistent store pointers for script access
        self.script.set_state_machine(&mut self.state_machine);
        self.script.set_persistent_store(&mut self.persistent_store);
        self.script.set_current_scene(&self.scene_path);

        // Set transition state for script access
        match &self.transition_phase {
            TransitionPhase::Idle => {
                self.script.set_transition_state(-1.0, "idle");
            }
            TransitionPhase::Exiting { elapsed, .. } => {
                self.script.set_transition_state(*elapsed as f64, "exiting");
            }
            TransitionPhase::Loading { .. } => {
                self.script.set_transition_state(1.0, "loading");
            }
            TransitionPhase::Entering { elapsed } => {
                self.script.set_transition_state(*elapsed as f64, "entering");
            }
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
        let screen_rect = self.egui_ctx.screen_rect();
        self.script.set_screen_size(screen_rect.width(), screen_rect.height());

        // Only run on_update when scripts are not paused
        if config.scripts == SystemPolicy::Run {
            self.script
                .update(&mut self.world, self.clock.delta_time)
                .unwrap_or_else(|e| eprintln!("Script error: {:?}", e));
        }

        // Apply script camera overrides (for non-FPS camera modes like chase camera)
        let (cam_pos_override, cam_target_override, cam_fov_override) = self.script.take_camera_overrides();
        if let Some(pos) = cam_pos_override {
            self.camera.position = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
        }
        if let Some(target) = cam_target_override {
            self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam_fov_override {
            self.camera.fov = fov;
        }

        // Update audio listener for script-driven cameras (chase cam, etc.)
        if !has_fps_player && cam_pos_override.is_some() {
            let cam_pos = self.camera.position;
            let dir = flint_core::Vec3::new(
                self.camera.target.x - cam_pos.x,
                self.camera.target.y - cam_pos.y,
                self.camera.target.z - cam_pos.z,
            );
            let yaw = dir.x.atan2(dir.z);
            let horiz = (dir.x * dir.x + dir.z * dir.z).sqrt();
            let pitch = (-dir.y).atan2(horiz);
            self.audio.update_listener(cam_pos, yaw, pitch);
        }

        // on_draw_ui() ALWAYS runs (pause menus, transition visuals need to draw)
        self.script.call_draw_uis(&mut self.world);

        let script_commands = self.script.drain_commands();
        self.process_script_commands(script_commands);

        // Clear state pointers after script calls
        self.script.clear_state_pointers();

        // Collect draw commands for this frame (scripts + data-driven UI)
        let mut commands = self.script.drain_draw_commands();
        let screen_rect = self.egui_ctx.screen_rect();
        let ui_commands = self.script.generate_ui_draw_commands(
            screen_rect.width(),
            screen_rect.height(),
        );
        commands.extend(ui_commands);
        self.draw_commands = commands;

        // Audio triggers from game events (skip when paused)
        if config.audio == SystemPolicy::Run {
            self.audio.process_events(&game_events, &self.world);
            self.audio
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }

        // Advance animations (skip when paused)
        if config.animation == SystemPolicy::Run {
            self.animation
                .update(&mut self.world, self.clock.delta_time)
                .ok();
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

        // Advance particle simulation (skip when paused)
        if config.particles == SystemPolicy::Run {
            self.particles
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }

        // Refresh renderer with updated transforms
        if let (Some(renderer), Some(context)) =
            (&mut self.scene_renderer, &self.render_context)
        {
            renderer.update_from_world(&self.world, &context.device);
        }

        // Upload particle instance data to GPU
        if let (Some(renderer), Some(context)) =
            (&mut self.scene_renderer, &self.render_context)
        {
            let sync_draw_data = self.particles.sync.draw_data();
            let render_draw_data: Vec<ParticleDrawData<'_>> = sync_draw_data
                .iter()
                .map(|d| {
                    let gpu_instances: &[ParticleInstanceGpu] =
                        bytemuck::cast_slice(bytemuck::cast_slice::<_, u8>(d.instances));
                    ParticleDrawData {
                        instances: gpu_instances,
                        texture: d.texture,
                        additive: d.blend_mode == flint_particles::ParticleBlendMode::Additive,
                    }
                })
                .collect();
            renderer.update_particles(&context.device, render_draw_data);
        }

        // Drain script post-processing overrides for this frame
        let (pp_vig, pp_bloom, pp_exp, pp_ca, pp_rb) = self.script.take_postprocess_overrides();
        self.pp_vignette_override = pp_vig;
        self.pp_bloom_override = pp_bloom;
        self.pp_exposure_override = pp_exp;
        self.pp_chromatic_aberration_override = pp_ca;
        self.pp_radial_blur_override = pp_rb;

        // Apply audio low-pass filter override from scripts
        if let Some(cutoff) = self.script.take_audio_overrides() {
            self.audio.set_filter_cutoff(cutoff);
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
                    if self.audio.engine.is_available() {
                        if let Err(e) = self.audio.engine.play_non_spatial(&name, volume, 1.0, false) {
                            eprintln!("[script] play_sound error: {:?}", e);
                        }
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
                    // Intercept transition completion signal
                    if name == "__transition_complete" {
                        match &self.transition_phase {
                            TransitionPhase::Exiting { target_scene, .. } => {
                                let target = target_scene.clone();
                                self.transition_phase = TransitionPhase::Loading {
                                    target_scene: target,
                                };
                            }
                            TransitionPhase::Entering { .. } => {
                                self.transition_phase = TransitionPhase::Idle;
                                println!("[transition] Transition complete");
                            }
                            _ => {}
                        }
                        continue;
                    }
                    self.physics.event_bus.push(GameEvent::Custom { name, data });
                }
                ScriptCommand::Log { level, message } => {
                    match level {
                        LogLevel::Info => println!("[script] {}", message),
                        LogLevel::Warn => eprintln!("[script warn] {}", message),
                        LogLevel::Error => eprintln!("[script error] {}", message),
                    }
                }
                ScriptCommand::EmitBurst { entity_id, count } => {
                    let eid = flint_core::EntityId(entity_id as u64);
                    self.particles.sync.queue_burst(eid, count as u32);
                }
                ScriptCommand::LoadScene { path } => {
                    if self.transition_phase.is_idle() {
                        println!("[transition] Starting exit transition → {}", path);
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::ReloadScene => {
                    if self.transition_phase.is_idle() {
                        let path = self.scene_path.clone();
                        println!("[transition] Reloading current scene");
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::PushState { name } => {
                    if self.state_machine.push_state(&name) {
                        println!("[state] Pushed '{}'", name);
                    } else {
                        eprintln!("[state] Unknown state template: '{}'", name);
                    }
                }
                ScriptCommand::PopState => {
                    if let Some(popped) = self.state_machine.pop_state() {
                        println!("[state] Popped '{}'", popped.name);
                    }
                }
                ScriptCommand::ReplaceState { name } => {
                    if self.state_machine.replace_state(&name) {
                        println!("[state] Replaced top with '{}'", name);
                    } else {
                        eprintln!("[state] Unknown state template: '{}'", name);
                    }
                }
            }
        }
    }

    /// Advance the transition phase based on elapsed time and script signals.
    fn advance_transition(&mut self) {
        match &self.transition_phase {
            TransitionPhase::Idle => {}
            TransitionPhase::Exiting { target_scene, elapsed } => {
                let new_elapsed = elapsed + self.clock.delta_time as f32;
                let target = target_scene.clone();
                self.transition_phase = TransitionPhase::Exiting {
                    target_scene: target.clone(),
                    elapsed: new_elapsed,
                };
                // Check if a complete_transition event was fired
                // (We use a sentinel event name to signal completion)
            }
            TransitionPhase::Loading { target_scene } => {
                let target = target_scene.clone();
                self.execute_scene_transition(&target);
                self.transition_phase = TransitionPhase::Entering { elapsed: 0.0 };
            }
            TransitionPhase::Entering { elapsed } => {
                let new_elapsed = elapsed + self.clock.delta_time as f32;
                self.transition_phase = TransitionPhase::Entering {
                    elapsed: new_elapsed,
                };
            }
        }
    }

    /// Unload the current scene and load a new one.
    fn execute_scene_transition(&mut self, target_scene: &str) {
        println!("[transition] Unloading current scene...");

        // Call on_scene_exit on all scripts
        self.script.set_state_machine(&mut self.state_machine);
        self.script.set_persistent_store(&mut self.persistent_store);
        self.script.call_scene_exits(&mut self.world);
        self.script.clear_state_pointers();

        // Clear all systems
        self.script.clear();
        self.audio.clear();
        self.physics.clear();
        self.animation.clear();
        self.particles.clear();

        // Clear world
        self.world = FlintWorld::new();

        // Clear transient rendering state
        self.skeletal_entity_assets.clear();
        self.ui_textures.clear();
        self.draw_commands.clear();

        println!("[transition] Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let new_scene_path = resolve_scene_path(&self.scene_path, target_scene);

        // Load schema registry
        let registry = if self.schema_paths.is_empty() {
            // Try default schemas/ dir
            flint_schema::SchemaRegistry::load_from_directory("schemas")
                .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
        } else {
            let existing: Vec<&str> = self.schema_paths.iter()
                .map(|s| s.as_str())
                .filter(|p| Path::new(p).exists())
                .collect();
            if existing.is_empty() {
                flint_schema::SchemaRegistry::new()
            } else {
                flint_schema::SchemaRegistry::load_from_directories(&existing)
                    .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
            }
        };

        // Parse and load scene
        match flint_scene::load_scene(&new_scene_path, &registry) {
            Ok((world, scene_file)) => {
                self.world = world;
                self.scene_path = new_scene_path.clone();
                self.skybox_path = scene_file.environment.as_ref()
                    .and_then(|env| env.skybox.clone());
                self.scene_post_process = scene_file.post_process.clone();
                self.scene_input_config = scene_file.scene.input_config.clone();
            }
            Err(e) => {
                eprintln!("[transition] Failed to load scene '{}': {:?}", new_scene_path, e);
                return;
            }
        }

        // Reload models
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            self.skeletal_entity_assets = load_models_from_world(
                &self.world,
                renderer,
                &mut self.animation,
                &context.device,
                &context.queue,
                &self.scene_path,
                self.catalog.as_ref(),
                self.content_store.as_ref(),
            );
            renderer.update_from_world(&self.world, &context.device);

            // Reload splines
            crate::spline_gen::load_splines(
                &self.scene_path,
                &mut self.world,
                renderer,
                Some(&mut self.physics),
                &context.device,
            );
            renderer.update_from_world(&self.world, &context.device);

            // Reload skybox
            if let Some(skybox_rel) = &self.skybox_path {
                let scene_dir = Path::new(&self.scene_path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let skybox_path = {
                    let p = scene_dir.join(skybox_rel);
                    if p.exists() {
                        p
                    } else if let Some(parent) = scene_dir.parent() {
                        parent.join(skybox_rel)
                    } else {
                        p
                    }
                };
                if skybox_path.exists() {
                    renderer.load_skybox(&context.device, &context.queue, &skybox_path);
                }
            }

            // Apply post-process config
            if let Some(pp_def) = &self.scene_post_process {
                use flint_render::PostProcessConfig;
                let mut config = PostProcessConfig::default();
                config.bloom_enabled = pp_def.bloom_enabled;
                config.bloom_intensity = pp_def.bloom_intensity;
                config.bloom_threshold = pp_def.bloom_threshold;
                config.vignette_enabled = pp_def.vignette_enabled;
                config.vignette_intensity = pp_def.vignette_intensity;
                config.exposure = pp_def.exposure;
                renderer.set_post_process_config(config);
            }
        }

        // Re-initialize systems
        self.physics
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Physics init: {:?}", e));

        load_audio_from_world(&self.world, &mut self.audio, &self.scene_path);
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Audio init: {:?}", e));

        load_animations_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Animation init: {:?}", e));

        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Particles init: {:?}", e));

        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| eprintln!("Script init: {:?}", e));

        // Call on_scene_enter on new scripts
        self.script.set_current_scene(&self.scene_path);
        self.script.set_state_machine(&mut self.state_machine);
        self.script.set_persistent_store(&mut self.persistent_store);
        self.script.call_scene_enters(&mut self.world);
        self.script.clear_state_pointers();

        // Recapture cursor if player exists
        if self.physics.character.player_entity().is_some() && !self.cursor_captured {
            self.capture_cursor();
        }

        println!("[transition] Scene loaded: {}", self.scene_path);
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
                    // Resize post-processing HDR buffer and bloom chain
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
                                KeyCode::F5 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config =
                                            renderer.post_process_config().clone();
                                        config.bloom_enabled = !config.bloom_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F6 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config =
                                            renderer.post_process_config().clone();
                                        config.enabled = !config.enabled;
                                        renderer.set_post_process_config(config);
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

/// Resolve a scene path relative to the current scene.
/// If target is already an absolute path or starts from a known root, use it directly.
/// Otherwise resolve relative to the current scene's directory.
fn resolve_scene_path(current_scene: &str, target: &str) -> String {
    let target_path = Path::new(target);
    if target_path.is_absolute() || target_path.exists() {
        return target.to_string();
    }

    let current_dir = Path::new(current_scene)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let resolved = current_dir.join(target);
    if resolved.exists() {
        return resolved.to_string_lossy().to_string();
    }

    // Try parent directory (game root)
    if let Some(parent) = current_dir.parent() {
        let parent_resolved = parent.join(target);
        if parent_resolved.exists() {
            return parent_resolved.to_string_lossy().to_string();
        }
    }

    // Return as-is, let scene loader report the error
    target.to_string()
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

// --- Input config helpers ---

fn resolve_input_paths(
    scene_path: &Path,
    scene_input_config: Option<&str>,
    cli_override: Option<&str>,
) -> InputConfigPaths {
    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));

    // Game default: look next to the scene, then parent (game root)
    let game_default = scene_input_config
        .map(|name| {
            let p = scene_dir.join(name);
            if p.exists() {
                p
            } else if let Some(parent) = scene_dir.parent() {
                parent.join(name)
            } else {
                p
            }
        })
        .or_else(|| {
            let candidate = scene_dir.join("config").join("input.toml");
            if candidate.exists() {
                Some(candidate)
            } else {
                scene_dir.parent().map(|p| p.join("config").join("input.toml")).filter(|p| p.exists())
            }
        });

    // User override: ~/.flint/input_{game_id}.toml (resolved later once game_id is known)
    // For now, try the project-local fallback
    let user_override = {
        let local = scene_dir.join(".flint").join("input.user.toml");
        if local.exists() {
            Some(local)
        } else {
            dirs::config_dir().map(|d| d.join("flint").join("input.user.toml")).filter(|p| p.exists())
        }
    };

    let cli = cli_override.map(PathBuf::from);

    InputConfigPaths {
        game_default,
        user_override,
        cli_override: cli,
    }
}

fn fallback_user_override_path(scene_path: &Path, game_id: &str) -> Option<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        let dir = config_dir.join("flint");
        let filename = if game_id.is_empty() || game_id == "flint" {
            "input.user.toml".to_string()
        } else {
            format!("input_{game_id}.toml")
        };
        return Some(dir.join(filename));
    }
    // Fallback to project-local
    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
    Some(scene_dir.join(".flint").join("input.user.toml"))
}

fn write_user_override_file(path: &Path, config: &InputConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FlintError::RuntimeError(format!(
                "failed to create directory '{}': {e}",
                parent.display()
            ))
        })?;
    }
    let toml_str = toml::to_string_pretty(config).map_err(|e| {
        FlintError::RuntimeError(format!("failed to serialize input config: {e}"))
    })?;
    std::fs::write(path, toml_str).map_err(|e| {
        FlintError::RuntimeError(format!(
            "failed to write input config '{}': {e}",
            path.display()
        ))
    })?;
    Ok(())
}

fn gamepad_id_to_u32(id: gilrs::GamepadId) -> u32 {
    // gilrs GamepadId is opaque; convert via usize
    let raw: usize = id.into();
    raw as u32
}
