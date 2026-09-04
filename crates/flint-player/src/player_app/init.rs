//! Construction, initialization, and asset-loading lifecycle for
//! `PlayerApp` — code-motion sibling of `mod.rs` (player_app decomposition;
//! see the decomposition ADR). Frame-time loading helpers that the render
//! path calls (`load_pending_sprites`) live here with the rest of the
//! loading surface.

use super::input_config::resolve_input_paths;
use super::scene_loading;
use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_particle_textures_from_world, load_scripts_from_world, load_sequences_from_world,
    load_sprite_animations_from_world, load_terrain_from_world_inner, register_node_animation_data,
    register_skeletal_data, resolve_procgen_assets,
};
use super::PlayerApp;
use super::TransitionPhase;
use anyhow::{Context, Result};
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_render::model_loader;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{
    GameClock, GameStateMachine, InputConfig, InputState, PersistentStore, RuntimeSystem,
};
use flint_script::context::DrawCommand;
use flint_script::ScriptSystem;
use gilrs::Gilrs;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

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
            music_session: None,
            music_exit_requested: false,
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
            scene_ambient: None,
            msaa_sample_count: 1,
            scene_diffuse_wrap: None,
            scene_oren_nayar: None,
            scene_sheen: None,
            scene_camera: None,
            scene_post_process: None,
            pp_vignette_override: None,
            pp_bloom_override: None,
            pp_exposure_override: None,
            pp_chromatic_aberration_override: None,
            pp_radial_blur_override: None,
            pp_ssao_intensity_override: None,
            pp_fog_density_override: None,
            pp_desaturation_override: None,
            pp_dof_strength_override: None,
            pp_dof_focus_distance_override: None,
            pp_dof_focus_range_override: None,
            pp_fog_color_override: None,
            pp_render_mode_override: None,
            pp_mode_params_override: None,
            pp_mode_was_active: false,
            #[cfg(feature = "debug-hud")]
            pp_debug_freeze: false,
            music_pp_base: None,
            music_pp_restore: None,
            scene_preload_audio: true,
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
            terrain: None,
            #[cfg(feature = "debug-hud")]
            debug_panels: Vec::new(),
            particles_render_enabled: true,
            show_stats: false,
            stats_frame_times: std::collections::VecDeque::new(),
            procgen_resolver: flint_procgen::ProcGenResolver::new(),
            loaded_chunks: HashMap::new(),
            #[cfg(target_os = "android")]
            android_gamepad: AndroidGamepadTracker::new(),
            #[cfg(target_os = "android")]
            android_axis_reader: None,
        }
    }

    /// Set the schema paths used for scene loading (preserved across transitions).
    pub fn set_schema_paths(&mut self, paths: Vec<String>) {
        self.schema_paths = paths;
    }

    /// Register a function that reads gamepad trigger/right-stick axes from the
    /// Android JNI bridge. Called each frame in `poll_gamepad_events()`.
    /// Returns [left_trigger, right_trigger, right_stick_x, right_stick_y].
    #[cfg(target_os = "android")]
    pub fn set_android_axis_reader(&mut self, reader: fn() -> [f32; 4]) {
        self.android_axis_reader = Some(reader);
    }

    /// Apply scene-level camera configuration from `CameraDef`
    pub(super) fn apply_camera_def(&mut self) {
        if let Some(cam) = &self.scene_camera {
            if cam.projection == "orthographic" {
                self.camera.orthographic = true;
                if cam.ortho_height > 0.0 {
                    self.camera.ortho_height = cam.ortho_height;
                }
            } else {
                self.camera.orthographic = false;
                self.camera.ortho_height = 0.0;
            }
            if let Some(pos) = cam.position {
                self.camera.position = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
            }
            if let Some(target) = cam.target {
                self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
            }
            if let Some(fov) = cam.fov {
                self.camera.fov = fov;
            }
            if let Some(near) = cam.near {
                self.camera.near = near;
            }
            if let Some(far) = cam.far {
                self.camera.far = far;
            }

            // Derive orbit parameters from position/target so update_orbit() stays consistent
            if cam.position.is_some() {
                let dir = self.camera.position - self.camera.target;
                let dist = dir.length();
                if dist > 0.001 {
                    self.camera.distance = dist;
                    let n = dir * (1.0 / dist);
                    self.camera.pitch = n.y.asin();
                    self.camera.yaw = n.x.atan2(n.z);
                }
            }
        }
    }

    pub(super) fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        // Window, then renderer + model/procgen loading (context and
        // renderer stay locals until the environment pass is done — they
        // are stored on self only once fully built, as before).
        let window = self.create_window(event_loop)?;
        let (render_context, mut scene_renderer) = self.init_render_and_models(&window)?;

        // Input bindings, gamepad backend, egui overlay
        self.init_input_and_egui(&window, &render_context);

        // Splines, ambient/wrap/sheen, skybox, terrain, camera, post config
        self.apply_scene_environment(&render_context, &mut scene_renderer);

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);

        // Physics → audio → music session → animation → particles → scripts
        // (audio before the scene-declared session, session before script
        // init so the conducted context exists from the scripts' first
        // frame — F3/F4 ordering).
        self.init_game_systems()?;

        // Capture cursor for first-person look (only if FPS player exists).
        // On Android, always set cursor_captured = true so touch input flows
        // without requiring a click-to-capture gate.
        #[cfg(target_os = "android")]
        {
            self.cursor_captured = true;
        }
        #[cfg(not(target_os = "android"))]
        if self.physics.has_player_entity() {
            self.capture_cursor();
        }

        Ok(())
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<Arc<Window>> {
        let window_attrs = Window::default_attributes().with_title("Flint Player");
        #[cfg(not(target_os = "android"))]
        let window_attrs = window_attrs.with_inner_size(PhysicalSize::new(1280, 720));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create player window")?,
        );

        if self.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        self.window = Some(window.clone());

        Ok(window)
    }

    fn init_render_and_models(
        &mut self,
        window: &Arc<Window>,
    ) -> Result<(RenderContext, SceneRenderer)> {
        // Initialize rendering
        let render_context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to initialize render context")?;

        self.camera.aspect = render_context.aspect_ratio();
        self.camera.fov = 70.0; // Slightly wider FOV for first-person

        let mut scene_renderer = SceneRenderer::new(
            &render_context,
            flint_render::RendererConfig {
                sample_count: self.msaa_sample_count,
                ..Default::default()
            },
        );

        // Rebuild component index as a safety net after scene loading
        self.world.rebuild_component_index();

        // Set scene_dir for font/texture resolution
        scene_renderer.scene_dir = Path::new(&self.scene_path)
            .parent()
            .map(|p| p.to_path_buf());

        // Load models from world (including skeletal data)
        let config = build_model_load_config(
            &self.scene_path,
            &self.world,
            self.catalog.as_ref(),
            self.content_store.as_ref(),
        );

        // Discover procgen specs and resolve unresolved assets before model loading
        {
            let scene_dir = Path::new(&self.scene_path)
                .parent()
                .unwrap_or(Path::new("."));
            let mut spec_dirs = vec![scene_dir.join("specs")];
            if let Some(parent) = scene_dir.parent() {
                spec_dirs.push(parent.join("specs"));
            }
            spec_dirs.push(scene_dir.join("models"));
            let dir_refs: Vec<&Path> = spec_dirs
                .iter()
                .filter(|d| d.is_dir())
                .map(|d| d.as_path())
                .collect();
            self.procgen_resolver.discover_and_index(&dir_refs);

            resolve_procgen_assets(
                &self.world,
                &mut self.procgen_resolver,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &config,
            );
        }

        let load_result = model_loader::load_models_from_world(
            &mut self.world,
            &mut scene_renderer,
            &render_context.device,
            &render_context.queue,
            &config,
        );
        register_skeletal_data(&load_result, &mut self.animation);
        register_node_animation_data(&load_result, &mut self.animation);
        self.skeletal_entity_assets = load_result.skinned_entities;
        scene_renderer.update_from_world(&self.world, &render_context.device);

        Ok((render_context, scene_renderer))
    }

    fn init_input_and_egui(&mut self, window: &Arc<Window>, render_context: &RenderContext) {
        // Load input configs with deterministic layering.
        self.configure_input_bindings()
            .unwrap_or_else(|e| tracing::warn!("Input config load error: {e:#}"));

        // Initialize gamepad backend (best-effort).
        self.gilrs = Gilrs::new().ok();

        // Initialize egui for HUD overlay
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window,
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
    }

    fn apply_scene_environment(
        &mut self,
        render_context: &RenderContext,
        scene_renderer: &mut SceneRenderer,
    ) {
        // Generate procedural geometry from spline + spline_mesh entities
        crate::spline_gen::load_splines(
            &self.scene_path,
            &mut self.world,
            scene_renderer,
            Some(&mut self.physics),
            &render_context.device,
        );

        // Refresh renderer with any new procedural meshes
        scene_renderer.update_from_world(&self.world, &render_context.device);

        // Scene-authored hemisphere ambient + diffuse wrap (absent = renderer default)
        if let Some((sky, ground)) = self.scene_ambient {
            scene_renderer.set_ambient(sky, ground);
        }
        if let Some(wrap) = self.scene_diffuse_wrap {
            scene_renderer.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = self.scene_oren_nayar {
            scene_renderer.set_oren_nayar(oren);
        }
        if let Some((color, strength)) = self.scene_sheen {
            scene_renderer.set_sheen(color, strength);
        }

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
                tracing::warn!("Skybox file not found: {}", skybox_path.display());
            }
        }

        // Load terrain from world
        #[cfg(feature = "debug-hud")]
        self.debug_panels.clear();
        self.load_terrain_from_world(
            &render_context.device,
            &render_context.queue,
            scene_renderer,
        );

        // Set terrain height callback for scripts
        self.update_terrain_height_fn();

        // Apply scene-level camera configuration
        self.apply_camera_def();
        self.apply_camera_tuning();

        // Apply scene-level post-processing config
        if let Some(pp_def) = &self.scene_post_process {
            scene_renderer
                .set_post_process_config(scene_loading::post_process_config_from_def(pp_def));
            scene_renderer.ensure_kuwahara_resources(&render_context.device, &render_context.queue);
            scene_renderer.ensure_fxaa_resources(&render_context.device);
        }
    }

    fn init_game_systems(&mut self) -> Result<()> {
        // Initialize physics
        self.physics
            .initialize(&mut self.world)
            .context("Failed to initialize physics")?;

        // Initialize audio
        load_audio_from_world(
            &self.world,
            &mut self.audio,
            &self.scene_path,
            self.scene_preload_audio,
        );
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        // Scene-declared music session (F3): started before script init so a
        // conducted context exists from the scripts' first frame (F4).
        self.start_music_session();

        // Initialize animation
        load_animations_from_world(&self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.scene_path, &mut self.animation);
        load_sequences_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        // Initialize particles: register `particles/*.particles.toml` first so
        // `particle_effect` components resolve on the first sync.
        flint_particles::load_particle_effects_from_world(&self.scene_path, &mut self.particles);
        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        self.load_particle_textures();

        // Initialize scripting (state_scope required so on_init can access persist store)
        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script.set_current_scene(&self.scene_path);
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script
                .initialize(&mut self.world)
                .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));
        }

        Ok(())
    }

    /// Scan world entities for `terrain` component, load heightmap, generate chunks,
    /// upload GPU resources, and register physics colliders.
    pub(super) fn load_terrain_from_world(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_renderer: &mut SceneRenderer,
    ) {
        let mut grass_info = None;
        load_terrain_from_world_inner(
            &self.world,
            &self.scene_path,
            device,
            queue,
            scene_renderer,
            &mut self.physics,
            &mut self.terrain,
            &mut grass_info,
        );

        // Create grass debug panel if grass was loaded
        #[cfg(feature = "debug-hud")]
        if let Some(info) = grass_info {
            let panel = flint_debug_ui::GrassDebugPanel::new(
                info.config,
                std::path::PathBuf::from(&self.scene_path),
                info.terrain_entity_name,
            );
            self.debug_panels.push(Box::new(panel));
        }
        #[cfg(not(feature = "debug-hud"))]
        let _ = grass_info;

        #[cfg(feature = "debug-hud")]
        {
            self.create_ocean_debug_panel();
            self.create_tod_debug_panel();
            self.create_weather_debug_panel();
            self.create_reality_debug_panel();
            self.create_visitor_debug_panel();
            self.create_dead_calm_debug_panel();
            self.create_camera_debug_panel();
            self.create_particles_debug_panel();
            // Rendering & Effects (ADR 0053): unconditional — every scene has
            // a renderer to tune. Registered closed; F4 summons it. show_freeze
            // is true here because player scripts drive post fields.
            self.debug_panels
                .push(Box::new(flint_debug_ui::RenderDebugPanel::new(true)));
        }
    }

    /// Update the terrain height callback on the script system.
    /// Call after terrain loading or clearing.
    pub(super) fn update_terrain_height_fn(&mut self) {
        if let Some((ref terrain, ref config)) = self.terrain {
            let heights = terrain.heightmap.clone_heights();
            let hm_w = terrain.heightmap.width;
            let hm_d = terrain.heightmap.depth;
            let world_w = config.width;
            let world_d = config.depth;
            let height_scale = config.height_scale;
            self.script
                .set_terrain_height_fn(Some(Box::new(move |x: f32, z: f32| {
                    // Convert world coords to normalized UV
                    let u = x / world_w;
                    let v = z / world_d;
                    // Bilinear sample from heights array
                    let fx = u * (hm_w as f32 - 1.0);
                    let fz = v * (hm_d as f32 - 1.0);
                    let ix = (fx as u32).min(hm_w - 2);
                    let iz = (fz as u32).min(hm_d - 2);
                    let tx = fx - ix as f32;
                    let tz = fz - iz as f32;
                    let idx = |col: u32, row: u32| -> f32 { heights[(row * hm_w + col) as usize] };
                    let h00 = idx(ix, iz);
                    let h10 = idx(ix + 1, iz);
                    let h01 = idx(ix, iz + 1);
                    let h11 = idx(ix + 1, iz + 1);
                    let h = h00 * (1.0 - tx) * (1.0 - tz)
                        + h10 * tx * (1.0 - tz)
                        + h01 * (1.0 - tx) * tz
                        + h11 * tx * tz;
                    h * height_scale
                })));
        } else {
            self.script.set_terrain_height_fn(None);
        }
    }

    pub(super) fn configure_input_bindings(&mut self) -> Result<()> {
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

    /// Load image files referenced by particle_emitter texture fields into
    /// the renderer's texture cache (no-op until renderer/context exist).
    pub(super) fn load_particle_textures(&mut self) {
        let (Some(renderer), Some(context)) =
            (self.scene_renderer.as_mut(), self.render_context.as_ref())
        else {
            return;
        };
        load_particle_textures_from_world(
            &self.particles,
            renderer,
            &context.device,
            &context.queue,
            &self.scene_path,
        );
    }

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
            scene_dir
                .parent()
                .map(|p| p.join("sprites").join(name))
                .unwrap_or_default(),
            scene_dir.join(name),
        ];

        for path in &candidates {
            if path.exists() {
                if let Ok(img) = image::open(path) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    let tex_handle =
                        self.egui_ctx
                            .load_texture(name, color_image, egui::TextureOptions::LINEAR);
                    self.ui_textures.insert(name.to_string(), tex_handle);
                    println!("Loaded UI sprite: {}", name);
                    return true;
                }
            }
        }

        tracing::warn!("UI sprite not found: {}", name);
        false
    }

    /// Pre-scan draw commands and load any sprite textures that haven't been loaded yet
    pub(super) fn load_pending_sprites(&mut self) {
        let sprite_names: Vec<String> = self
            .draw_commands
            .iter()
            .filter_map(|cmd| {
                if let DrawCommand::Sprite { name, .. } = cmd {
                    if !self.ui_textures.contains_key(name.as_str()) {
                        Some(name.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        for name in sprite_names {
            self.load_ui_texture(&name);
        }
    }
}
