//! Construction, initialization, and asset-loading lifecycle for
//! `PlayerApp` — code-motion sibling of `mod.rs` (player_app decomposition;
//! see the decomposition ADR). Frame-time loading helpers that the render
//! path calls (`load_pending_sprites`) live here with the rest of the
//! loading surface.

use super::input_config::{read_user_override_file, resolve_input_paths, resolve_user_override};
use super::scene_loading;
use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_particle_textures_from_world, load_scripts_from_world, load_sequences_from_world,
    load_sprite_animations_from_world, load_terrain_from_world_inner, register_node_animation_data,
    register_skeletal_data,
};
#[cfg(target_os = "android")]
use super::AndroidGamepadTracker;
use super::TransitionPhase;
use super::{CaptureConfig, PlayerApp};
use anyhow::{Context, Result};
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_core::vfs;
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
use flint_input_capture::pad::Gilrs;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

impl PlayerApp {
    /// Why the run failed, once `run_app` has returned: the player didn't
    /// start, or a run that asked for a screenshot doesn't have it (#639).
    /// A binary exits non-zero on `Some`, so a capture that left no file
    /// can't pass for one that did.
    pub fn run_failure(&self) -> Option<String> {
        if let Some(e) = &self.init_error {
            return Some(format!("the player failed to start: {e}"));
        }
        self.capture.missing()
    }

    pub fn new(
        world: FlintWorld,
        scene_path: String,
        fullscreen: bool,
        input_config_override: Option<String>,
        scene_input_config: Option<String>,
    ) -> Self {
        // The profiler's client, before the first scope (ADR 0109). Tracy's
        // viewer connects to it while the game runs; starting it twice is
        // harmless.
        #[cfg(feature = "profile")]
        profiling::tracy_client::Client::start();
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
            camera_cut: Default::default(),
            style_tweens: Default::default(),
            style_reader_generation: None,
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            draw_commands: Vec::new(),
            world_texts: Vec::new(),
            world_text_pipeline: None,
            ui_textures: HashMap::new(),
            ui_fonts: HashSet::new(),
            ui_fonts_warned: HashSet::new(),
            text_halos: Default::default(),
            ui_fonts_root: None,
            ui_fonts_installed: false,
            egui_fonts_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            catalog: AssetCatalog::load_from_directory("assets").ok(),
            content_store: Some(ContentStore::new(".flint/assets")),
            fullscreen,
            cursor_captured: false,
            window_focused: true,
            real_pad_slots: Default::default(),
            key_modifiers: Default::default(),
            skybox_path: None,
            scene_ambient: None,
            msaa_sample_count: 1,
            procgen_disk_cache: cfg!(not(target_arch = "wasm32")),
            scene_file_writable: cfg!(not(target_arch = "wasm32")),
            procgen_mode: super::procgen_assets::ProcGenMode::Immediate,
            render_scale: Default::default(),
            dynamic_scale: Default::default(),
            quality_overrides: String::new(),
            pixels_per_point: None,
            look: None,
            quality: Default::default(),
            frame_pacer: None,
            hold_at: None,
            scene_diffuse_wrap: None,
            scene_shadows: true,
            scene_default_lights: true,
            scene_oren_nayar: None,
            scene_sheen: None,
            scene_camera: None,
            scene_post_process: None,
            style_specs: Vec::new(),
            style_time: None,
            scene_living: None,
            living_blend: Default::default(),
            scene_wooze: None,
            wooze: None,
            wooze_time: 0.0,
            wooze_missing_warned: false,
            shallow_wooze: Default::default(),
            boil_clock: Default::default(),
            grass_wooze: Default::default(),
            living_tune: Default::default(),
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
            pending_look: None,
            music_pp_base: None,
            music_pp_restore: None,
            scene_preload_audio: true,
            audio_vfs_generation: flint_core::vfs::generation(),
            window_size: (1280, 720),
            window_options: crate::WindowArgs::default(),
            capture: CaptureConfig::default(),
            expression_feed: None,
            exit_requested: false,
            init_error: None,
            input_config_override,
            scene_input_config,
            input_config_paths: None,
            user_override_config: InputConfig {
                version: 1,
                game_id: String::new(),
                actions: Default::default(),
                touch_controls: None,
            },
            pending_rebind: None,
            gilrs: None,
            state_machine: GameStateMachine::new(),
            persistent_store: PersistentStore::new(),
            persist_debounce: flint_runtime::SaveDebounce::default(),
            transition_phase: TransitionPhase::Idle,
            schema_paths: Vec::new(),
            terrain: None,
            #[cfg(feature = "debug-hud")]
            debug_panels: Vec::new(),
            particles_render_enabled: true,
            show_stats: false,
            stats_frame_times: std::collections::VecDeque::new(),
            stats_gpu_timing: false,
            perf: None,
            procgen_resolver: flint_procgen::ProcGenResolver::new(),
            loaded_chunks: HashMap::new(),
            #[cfg(target_os = "android")]
            android_gamepad: AndroidGamepadTracker::new(),
            #[cfg(target_os = "android")]
            android_axis_reader: None,
            extensions: Vec::new(),
            egui_input: Default::default(),
            #[cfg(target_arch = "wasm32")]
            pending_render: Default::default(),
            #[cfg(target_arch = "wasm32")]
            event_proxy: None,
            #[cfg(target_arch = "wasm32")]
            pointer_lock: Default::default(),
        }
    }

    /// Install a game's extension. Its script API is registered right away,
    /// so add extensions before the event loop starts; after that, scripts
    /// already compiled still see the new functions, because Rhai resolves
    /// calls when they run. See `PlayerExtension` for when each hook runs.
    pub fn add_extension(&mut self, mut extension: Box<dyn crate::extension::PlayerExtension>) {
        extension.register_script_api(self.script.rhai_engine_mut());
        self.extensions.push(extension);
        // A late extension still gets its `fonts` hook.
        if self.ui_fonts_installed {
            self.ui_fonts_installed = false;
            self.refresh_project_fonts();
        }
        // And its debug panels, before the next scene load builds them again.
        #[cfg(feature = "debug-hud")]
        self.create_extension_debug_panels();
    }

    /// A handle on the camera scripts' `world_to_screen` uses, for native
    /// functions an extension registers (ADR 0087). Take it before
    /// `add_extension` and move it into the extension, whose
    /// `register_script_api` closures clone it. Called from a script, it
    /// answers with the camera this frame renders with, as
    /// `world_to_screen` does; from `pre_update`, last frame's, like
    /// `ExtensionCtx::camera`.
    pub fn projector(&self) -> crate::Projector {
        self.script.projector()
    }

    /// Take the scene file's player settings: audio preload, skybox,
    /// ambient and shading levers, camera and post-processing. Both entries
    /// (`main.rs` and the web's `flint_web.rs`) call it after `new`.
    pub fn apply_scene_settings(&mut self, scene_file: &flint_scene::SceneFile) {
        self.scene_preload_audio = scene_file.scene.preload_audio;

        // No shadows, no built-in lights (#627)
        let env = scene_file.environment.as_ref();
        self.scene_shadows = env.is_none_or(|e| e.casts_shadows());
        self.scene_default_lights = env.is_none_or(|e| e.uses_default_lights());

        // Skybox path + ambient from scene environment settings
        if let Some(env) = &scene_file.environment {
            self.skybox_path = env.skybox.clone();
            if env.ambient_sky.is_some() || env.ambient_ground.is_some() {
                self.scene_ambient = Some((
                    env.ambient_sky
                        .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_SKY),
                    env.ambient_ground
                        .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_GROUND),
                ));
            }
            self.scene_diffuse_wrap = env.diffuse_wrap;
            self.scene_oren_nayar = env.oren_nayar;
            self.scene_sheen = env
                .sheen_strength
                .map(|s| (env.sheen_color.unwrap_or([1.0; 3]), s));
        }

        self.scene_camera = scene_file.camera.clone();
        // User styles first: the post_process tables are read against the
        // registry, and a table for an unloaded style is dropped
        super::scene_loading::load_user_styles(&self.scene_path);
        self.set_scene_post_process(scene_file.post_process.clone());
        self.scene_living = scene_file.living.clone();
        self.scene_wooze = scene_file.wooze.clone();
    }

    /// Take a scene's `[post_process]` block, with the look it names
    /// (`look = "graphite"`, #356) laid under its own keys and `--look`
    /// (#435) over them. A look that can't be found is warned about and the
    /// block drawn without it. Call after the scene's user styles are
    /// loaded (a look may name one).
    pub fn set_scene_post_process(&mut self, def: Option<flint_scene::PostProcessDef>) {
        self.scene_post_process =
            super::scene_loading::scene_post_process(self.look.as_deref(), &self.scene_path, def);
    }

    /// The look the scene draws with now: the one it names, or the one a
    /// script or F4 chose since (#356). `None` for the scene's keys alone.
    pub fn current_look(&self) -> Option<&str> {
        self.scene_post_process.as_ref()?.look.as_deref()
    }

    /// Lay `look` under the scene's `[post_process]` keys in place of the
    /// one it has (`None` or `""`: no look, the scene's keys alone), and
    /// draw with it from the next frame (#356). Script post overrides keep
    /// stamping their fields over it, as they do over the scene's. A scene
    /// with no `[post_process]` block takes the look over the defaults.
    pub fn set_look(&mut self, look: Option<&str>) -> Result<(), String> {
        let base = match &self.scene_post_process {
            Some(def) => def.clone(),
            None => flint_scene::PostProcessDef::from_authored(toml::Table::new())
                .map_err(|e| e.to_string())?,
        };
        let def = flint_render::look::resolve(&base, look, &self.scene_path)?;
        self.scene_post_process = Some(def);
        let config = self.scene_post_config();
        if let (Some(renderer), Some(context), Some(config)) = (
            self.scene_renderer.as_mut(),
            self.render_context.as_ref(),
            config,
        ) {
            renderer.set_post_process_config(config);
            renderer.ensure_style_resources(&context.device);
            renderer.ensure_fxaa_resources(&context.device);
        }
        #[cfg(feature = "debug-hud")]
        self.refresh_style_panels_after_look();
        Ok(())
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

    /// The post-processing config a scene starts with: its `[post_process]`
    /// with the `--style` words written over it (#328). `None` when the
    /// scene authors no block and there are no words, so the renderer keeps
    /// what it has, as before.
    pub fn scene_post_config(&self) -> Option<flint_render::PostProcessConfig> {
        if self.scene_post_process.is_none() && self.style_specs.is_empty() {
            return None;
        }
        let mut config = self
            .scene_post_process
            .as_ref()
            .map(scene_loading::post_process_config_from_def)
            .unwrap_or_default();
        for e in flint_render::stylize::apply_style_specs(&mut config, &self.style_specs) {
            tracing::warn!("{e}");
        }
        Some(config)
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

    /// The first `resumed`: the window, then the GPU device. A desktop
    /// blocks on the device and finishes here. A browser can't block, so on
    /// the web the device request runs as a promise and `finish_initialize`
    /// follows when it lands (see `web_startup.rs`).
    pub(super) fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = self.create_window(event_loop)?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let render_context = pollster::block_on(RenderContext::new(window.clone()))
                .context("Failed to initialize render context")?;
            self.finish_initialize(window, render_context)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.request_render_context(window);
            Ok(())
        }
    }

    /// Everything after the device: renderer, models, input, egui, the
    /// scene's environment and the game systems.
    pub(super) fn finish_initialize(
        &mut self,
        window: Arc<Window>,
        render_context: RenderContext,
    ) -> Result<()> {
        // Renderer + model/procgen loading (context and renderer stay locals
        // until the environment pass is done — they are stored on self only
        // once fully built, as before).
        let (render_context, mut scene_renderer) = self.init_render_and_models(render_context)?;

        // Input bindings, gamepad backend, egui overlay
        self.init_input_and_egui(&window, &render_context);

        // Project fonts (fonts/*.ttf|otf) and the real text measurer for scripts
        self.refresh_project_fonts();
        self.install_text_measurer();

        // Splines, ambient/wrap/sheen, skybox, terrain, camera, post config
        self.apply_scene_environment(&render_context, &mut scene_renderer);

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);
        self.update_vsync_pacing();
        // Quality settings (FlintKart#55) before any script's on_init, so
        // get_quality() and gpu_info() answer from the first frame.
        self.init_quality();

        // Saved persistent store (save/persist.toml) before any script's
        // on_init runs, so persist_get sees last session's values.
        self.load_persistent_store();

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
        // In a browser the page's canvas is the window, sized by its CSS.
        #[cfg(target_arch = "wasm32")]
        let window_attrs = super::web_startup::canvas_attributes(window_attrs);
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        let window_attrs = window_attrs.with_inner_size(PhysicalSize::new(
            self.window_size.0.max(1),
            self.window_size.1.max(1),
        ));
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        let window_attrs = self.window_options.apply(window_attrs);

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
        render_context: RenderContext,
    ) -> Result<(RenderContext, SceneRenderer)> {
        self.camera.aspect = render_context.aspect_ratio();
        self.camera.fov = 70.0; // Slightly wider FOV for first-person

        let mut scene_renderer = SceneRenderer::new(
            &render_context,
            flint_render::RendererConfig {
                sample_count: self.msaa_sample_count,
                ..Default::default()
            },
        );
        // FLINT_SHADOWS / FLINT_SHADOW_RES and the rest of the quality
        // settings are applied by init_quality once the renderer is stored.

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

        // The procgen disk tier (#181), set up once, before the first scene's
        // assets are resolved.
        if self.procgen_resolver.disk_stats().is_none() {
            let scene_dir = Path::new(&self.scene_path)
                .parent()
                .unwrap_or(Path::new("."));
            if let Some(disk) = self
                .procgen_disk_cache
                .then(|| procgen_disk_config(scene_dir))
                .flatten()
            {
                println!("[procgen] disk cache: {}", disk.dir.display());
                self.procgen_resolver = flint_procgen::ProcGenResolver::with_config(
                    flint_procgen::ProcGenCacheConfig {
                        disk: Some(disk),
                        ..Default::default()
                    },
                );
            }
        }

        // Procgen before the model loader: catalog > procgen > file (#32).
        self.procgen_resolver
            .set_living_defaults(super::living_props::defaults_of(self.scene_living.as_ref()));
        self.wooze = super::living_props::field_of(self.scene_wooze.as_ref());
        self.wooze_time = 0.0;
        super::shallow_wooze::warn_without_a_field(&self.world, self.wooze.as_ref());
        self.boil_clock = Default::default();
        self.grass_wooze = Default::default();
        self.living_tune.for_new_scene();
        super::procgen_assets::resolve_scene(
            &self.scene_path,
            &mut self.world,
            &mut self.procgen_resolver,
            self.procgen_mode,
            super::procgen_assets::GpuSink {
                renderer: &mut scene_renderer,
                device: &render_context.device,
                queue: &render_context.queue,
                config: &config,
            },
        );

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

        // Initialize egui for HUD overlay. egui assumes textures of at most
        // 2048 px a side until told otherwise, and panics on a wider one
        // (Float's 2713 px title). Give it the device's limit, both for the
        // passes egui-winit feeds it and now, since UI sprites load before
        // the first pass.
        let max_texture_side = render_context.device.limits().max_texture_dimension_2d as usize;
        self.egui_ctx
            .input_mut(|i| i.max_texture_side = max_texture_side);
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(max_texture_side),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &render_context.device,
            render_context.hud_format,
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
            &render_context.queue,
        );

        // Refresh renderer with any new procedural meshes
        scene_renderer.update_from_world(&self.world, &render_context.device);

        scene_renderer.set_scene_shadows(self.scene_shadows);
        scene_renderer.set_default_lights(self.scene_default_lights);
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
                if vfs::exists(&p) {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join(skybox_rel)
                } else {
                    p
                }
            };

            if vfs::exists(&skybox_path) {
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
        if let Some(config) = self.scene_post_config() {
            scene_renderer.set_post_process_config(config);
            scene_renderer.ensure_style_resources(&render_context.device);
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
            // In the browser the audio may already be here at start, and
            // decoding it all in one frame is the stall #124 removed:
            // queue it. The desktop decodes up front, as a load can afford.
            cfg!(target_arch = "wasm32"),
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
            )
            .with_component(info.component);
            self.debug_panels.push(Box::new(panel));
        }
        #[cfg(not(feature = "debug-hud"))]
        let _ = grass_info;
        super::shallow_wooze::warn_grass_without_a_field(
            scene_renderer.grass_config(),
            self.wooze.as_ref(),
        );

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
            self.create_living_debug_panel();
            self.create_styles_debug_panel();
            // Rendering & Effects (ADR 0053): unconditional, F4 summons it
            self.create_render_debug_panel();
            // The game's own (ADR 0107), after the engine's.
            self.create_extension_debug_panels();
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

        let mut paths = resolve_input_paths(
            Path::new(&self.scene_path),
            self.scene_input_config.as_deref(),
            self.input_config_override.as_deref(),
        );

        if let Some(path) = &paths.game_default {
            if vfs::exists(path) {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load game input config '{}'", path.display())
                })?;
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge game input config")?;
            }
        }

        // Now the game default has set the game id, find the player's own
        // rebinds: they're saved under it.
        paths.user_override =
            resolve_user_override(Path::new(&self.scene_path), &self.input.config().game_id);
        // The player's copy (crate::user_files, localStorage on the web), else
        // one the game ships in its vfs bundle.
        if let Some(path) = &paths.user_override {
            let cfg = read_user_override_file(path).with_context(|| {
                format!("failed to load user input config '{}'", path.display())
            })?;
            if let Some(cfg) = cfg {
                self.user_override_config = cfg.clone();
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge user input config")?;
            }
        }

        if let Some(path) = &paths.cli_override {
            if vfs::exists(path) {
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

    /// (Re)load `<project>/fonts/` into egui when the project root changed
    /// since the last scan (initial load and scene transitions). Extensions'
    /// `fonts` hooks edit the definitions before they're installed.
    pub(super) fn refresh_project_fonts(&mut self) {
        let root = super::fonts::find_fonts_dir(&self.scene_path);
        if self.ui_fonts_installed && self.ui_fonts_root == root {
            return; // same project root as the last scan
        }
        let mut defs = super::fonts::project_font_definitions(&self.scene_path);
        for ext in self.extensions.iter_mut() {
            ext.fonts(&mut defs);
        }
        self.ui_fonts = super::fonts::script_families(&defs);
        self.egui_ctx.set_fonts(defs);
        self.ui_fonts_warned.clear();
        self.ui_fonts_root = root;
        self.ui_fonts_installed = true;
    }

    /// Give scripts a `measure_text` that lays text out with the same font
    /// stack the HUD draws with. Before egui's first pass (fonts not built
    /// yet) and for families not installed in the *active* definitions,
    /// the closure degrades gracefully instead of panicking.
    pub(super) fn install_text_measurer(&mut self) {
        let ctx = self.egui_ctx.clone();
        let ready = self.egui_fonts_ready.clone();
        self.script.set_text_measurer(Some(Arc::new(
            move |text: &str, size: f32, font: Option<&str>, spacing: f32| {
                if !ready.load(std::sync::atomic::Ordering::Relaxed) {
                    let chars = text.chars().count() as f32;
                    return (chars * size * 0.6 + spacing * (chars - 1.0).max(0.0), size);
                }
                ctx.fonts(|f| {
                    let family = font
                        .map(|n| egui::FontFamily::Name(n.into()))
                        .filter(|fam| f.lock().fonts.definitions().families.contains_key(fam))
                        .unwrap_or(egui::FontFamily::Proportional);
                    let job = super::fonts::text_layout_job(
                        text,
                        egui::FontId::new(size, family),
                        egui::Color32::WHITE,
                        spacing,
                    );
                    let galley = f.layout_job(job);
                    (galley.size().x, galley.size().y)
                })
            },
        )));
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
            if vfs::exists(path) {
                if let Ok(img) = flint_render::open_image(path) {
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
            .filter_map(|item| {
                if let DrawCommand::Sprite { name, .. } = &item.cmd {
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

/// Where the procgen disk tier goes for a scene in `scene_dir` (#181):
/// `FLINT_PROCGEN_CACHE=<dir>` if set (`0`, `off` or empty: no disk tier),
/// else `.flint/procgen-cache/` in the project, the scene folder's parent
/// when that holds `schemas/` or `specs/` (a `levels/` layout, as in
/// `showcase_game`), otherwise the scene folder itself. Until projects have
/// a root marker and a config file (#179), that guess is the project.
fn procgen_disk_config(scene_dir: &Path) -> Option<flint_procgen::DiskCacheConfig> {
    if let Ok(value) = std::env::var("FLINT_PROCGEN_CACHE") {
        return match value.trim() {
            "" | "0" | "off" | "false" => None,
            dir => Some(flint_procgen::DiskCacheConfig::new(dir)),
        };
    }
    let project = scene_dir
        .parent()
        .filter(|p| vfs::is_dir(p.join("schemas")) || vfs::is_dir(p.join("specs")))
        .unwrap_or(scene_dir);
    Some(flint_procgen::DiskCacheConfig::in_project(project))
}

#[cfg(test)]
mod style_word_tests {
    use super::PlayerApp;

    fn app() -> PlayerApp {
        PlayerApp::new(
            flint_ecs::FlintWorld::new(),
            "style_words.scene.toml".into(),
            false,
            None,
            None,
        )
    }

    fn scene(post: &str) -> flint_scene::PostProcessDef {
        let doc: toml::Table = post.parse().unwrap();
        doc["post_process"].clone().try_into().unwrap()
    }

    #[test]
    fn without_words_or_a_block_the_renderer_keeps_its_config() {
        assert!(app().scene_post_config().is_none());
    }

    #[test]
    fn style_words_win_over_every_scene_they_meet() {
        let mut app = app();
        app.style_specs = vec!["ascii:cell=12".into()];

        // A scene with no [post_process] still gets the style
        let config = app.scene_post_config().unwrap();
        assert!(config.styles.is_enabled("ascii"));
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));

        // A scene that authors the same style, and keeps its own bloom:
        // the word wins on the style and leaves the rest alone. This is
        // what a transition re-reads (#328)
        app.scene_post_process = Some(scene(
            "[post_process]\nbloom_intensity = 0.3\n\
             [post_process.styles.ascii]\ncell = 6\nramp = \"blocks\"\n",
        ));
        let config = app.scene_post_config().unwrap();
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));
        assert_eq!(config.styles.option("ascii", "ramp"), Some("blocks"));
        assert!((config.bloom_intensity - 0.3).abs() < 1e-6);

        // ascii:off turns off a style the scene has on
        app.style_specs = vec!["ascii:off".into()];
        assert!(!app.scene_post_config().unwrap().styles.is_enabled("ascii"));
    }

    fn loaded(scene_text: &str) -> flint_scene::PostProcessDef {
        let mut def = scene(scene_text);
        def.authored = flint_scene::PostProcessDef::authored_table(scene_text);
        def
    }

    /// #356: a scene's look is laid under its keys on load, and a script
    /// (or F4) swaps it for another, or takes it off, from the scene's
    /// keys each time.
    #[test]
    fn a_scene_look_loads_and_set_look_swaps_it() {
        let mut app = app();
        app.set_scene_post_process(Some(loaded(
            "[post_process]\nlook = \"graphite\"\nbloom_intensity = 0.3\n",
        )));
        assert_eq!(app.current_look(), Some("graphite"));
        let config = app.scene_post_config().unwrap();
        assert!(config.styles.is_enabled("pencil"), "graphite's pencil");
        assert!(
            (config.bloom_intensity - 0.3).abs() < 1e-6,
            "the scene's key"
        );

        app.set_look(Some("matrix")).unwrap();
        assert_eq!(app.current_look(), Some("matrix"));
        let config = app.scene_post_config().unwrap();
        assert!(config.styles.is_enabled("ascii"), "matrix is ASCII");
        assert!(!config.styles.is_enabled("pencil"), "graphite is gone");
        assert!((config.bloom_intensity - 0.3).abs() < 1e-6);

        app.set_look(None).unwrap();
        assert_eq!(app.current_look(), None);
        let config = app.scene_post_config().unwrap();
        assert!(!config.styles.is_enabled("ascii"));
        assert!((config.bloom_intensity - 0.3).abs() < 1e-6);

        let err = app.set_look(Some("no_such_look")).unwrap_err();
        assert!(err.contains("unknown look"), "{err}");
        assert_eq!(app.current_look(), None, "a failed look changes nothing");
    }

    #[test]
    fn a_scene_without_a_block_still_takes_a_look() {
        let mut app = app();
        app.set_look(Some("ink_wash")).unwrap();
        assert_eq!(app.current_look(), Some("ink_wash"));
        assert!(app.scene_post_config().is_some());
    }
}
