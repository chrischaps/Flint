//! Construction, initialization, and asset-loading lifecycle for
//! `PlayerApp` — code-motion sibling of `mod.rs` (player_app decomposition;
//! see the decomposition ADR). Frame-time loading helpers that the render
//! path calls (`load_pending_sprites`) live here with the rest of the
//! loading surface.

use super::input_config::{read_user_override_file, resolve_input_paths, resolve_user_override};
use super::scene_loading;
use super::scene_loading::load_particle_textures_from_world;
use super::state::{AssetSources, InputLayers};
#[cfg(target_os = "android")]
use super::AndroidGamepadTracker;
use super::TransitionPhase;
use super::{CaptureConfig, LaunchOptions, PlayerApp, SceneSettings};
use anyhow::{Context, Result};
use flint_audio::AudioSystem;
use flint_core::vfs;
use flint_ecs::FlintWorld;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::pad::Gilrs;
use flint_runtime::{InputConfig, InputState};
use flint_script::context::DrawCommand;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

impl PlayerApp {
    /// Why the run failed, once `run_app` has returned: the player didn't
    /// start, a run that asked for a screenshot doesn't have it (#639), or
    /// a script failed under `--strict-scripts` (#693), or the run hitched
    /// more than `--max-hitches` allows (#1079). A binary exits
    /// non-zero on `Some`, so a capture that left no file can't pass for
    /// one that did, nor a broken game for a working one.
    pub fn run_failure(&self) -> Option<String> {
        if let Some(e) = &self.outcome.init_error {
            return Some(format!("the player failed to start: {e}"));
        }
        if let Some(e) = &self.outcome.scene_failure {
            return Some(e.clone());
        }
        if let Some(missing) = self.capture.missing() {
            return Some(missing);
        }
        // `--dump-world` (#713): every time it asked for, or why not. A
        // dump never handed over (no frame ran) has taken none.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dump = self
                .core
                .subsystems
                .find::<super::world_dump::WorldDump>()
                .or(self.launch.world_dump.as_ref());
            if let Some(missing) = dump.and_then(|d| d.missing()) {
                return Some(missing);
            }
            // `--expression-listen`'s port, taken when it opened (#584)
            let feed = self
                .core
                .subsystems
                .find::<super::expression_feed::ExpressionSubsystem>();
            if let Some(failure) = feed.and_then(|f| f.failure()) {
                return Some(failure);
            }
        }
        if self.launch.strict_scripts {
            if let Some(failure) = self.core.script.errors().strict_failure() {
                return Some(failure);
            }
        }
        // An input script's expect or wait that failed (#714)
        if let Some(failure) = self
            .capture
            .input_script
            .as_ref()
            .and_then(|s| super::input_checks::failure(s.outcomes()))
        {
            return Some(failure);
        }
        // A capture asked for and never written (#1086)
        if let Some(failure) = self.frame_captures.failure() {
            return Some(failure);
        }
        // A recording that stopped (#583)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(failure) = self.frame_grabs.failure() {
            return Some(failure);
        }
        // A trace that couldn't be written (#1097)
        if let Some(failure) = self.trace_failure() {
            return Some(failure);
        }
        // `--perf-report` that couldn't be written (#1093)
        if let Some(failure) = self.perf_report_failure() {
            return Some(failure);
        }
        // `--max-hitches` and `--max-hitches-startup` (#1079)
        self.stats.watch.failure()
    }

    pub fn new(
        world: FlintWorld,
        scene_path: String,
        fullscreen: bool,
        input_config_override: Option<String>,
        scene_input_config: Option<String>,
    ) -> Self {
        Self::with_audio(
            world,
            scene_path,
            fullscreen,
            input_config_override,
            scene_input_config,
            // Opening the sound device can take a second (#1092)
            flint_core::load_report::timed(
                flint_core::load_report::Stage::AudioDevice,
                String::new,
                AudioSystem::new,
            ),
        )
    }

    /// [`new`](Self::new) with its sound given: `AudioSystem::silent()`
    /// for a run that must open no device (a headless playtest, #714).
    pub fn with_audio(
        world: FlintWorld,
        scene_path: String,
        fullscreen: bool,
        input_config_override: Option<String>,
        scene_input_config: Option<String>,
        audio: AudioSystem,
    ) -> Self {
        // The profiler's client, before the first scope (ADR 0109). Tracy's
        // viewer connects to it while the game runs; starting it twice is
        // harmless.
        #[cfg(feature = "profile")]
        profiling::tracy_client::Client::start();
        // Before the file source may be final (the web installs its bundle
        // later): `refresh_project` asks again when the scene loads.
        let project = flint_core::project::Project::for_scene(Path::new(&scene_path));
        Self {
            core: super::GameCore::new(world, scene_path, project),
            input: InputState::new(),
            audio,
            launch: LaunchOptions {
                fullscreen,
                ..Default::default()
            },
            scene: SceneSettings {
                input_config: scene_input_config,
                ..Default::default()
            },
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            style: Default::default(),
            quality: Default::default(),
            frame_pacer: None,
            skeletal_entity_assets: HashMap::new(),
            hud: Default::default(),
            assets: AssetSources::default(),
            focus: Default::default(),
            post: Default::default(),
            audio_vfs_generation: flint_core::vfs::generation(),
            capture: CaptureConfig::default(),
            frame_captures: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            frame_grabs: Default::default(),
            outcome: Default::default(),
            #[cfg(test)]
            test_gpu: None,
            offscreen: None,
            input_layers: InputLayers::new(input_config_override),
            gilrs: None,
            pad_rumble: flint_runtime::PadRumble::new(),
            pads_lent: false,
            saves: Default::default(),
            transition_phase: TransitionPhase::Idle,
            schema_dirs: Default::default(),
            schemas: Default::default(),
            terrain: None,
            scene_binding: None,
            #[cfg(feature = "debug-hud")]
            debug_panels: Vec::new(),
            particles_render_enabled: true,
            stats: Default::default(),
            loaded_chunks: HashMap::new(),
            late_models: Default::default(),
            #[cfg(target_os = "android")]
            android_gamepad: AndroidGamepadTracker::new(),
            #[cfg(target_os = "android")]
            android_axis_reader: None,
            extensions: Vec::new(),
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
        extension.register_script_api(self.core.script.rhai_engine_mut());
        extension.declare_script_api(&mut self.core.script.script_api());
        #[cfg(feature = "procgen")]
        extension.register_generators(self.core.procgen_resolver.registry_mut());
        // The game's own script modules (#754): declared, and enabled for
        // the run beside whatever the scene's subsystems enable
        for module in extension.script_modules() {
            self.core.script.declare_module(*module);
            if !self.core.extension_modules.contains(&module.name()) {
                self.core.extension_modules.push(module.name());
            }
        }
        if !extension.script_modules().is_empty() {
            self.core.enable_script_modules();
        }
        // The game's Rust styles (#419), before any scene's tables are read
        for descriptor in extension.styles() {
            if let Err(e) = self.core.styles.register(descriptor) {
                tracing::error!("player extension style: {e}");
            }
        }
        // A renderer made already draws them from now on
        if let Some(renderer) = self.scene_renderer.as_mut() {
            renderer.set_styles(self.core.styles.clone());
        }
        self.extensions.push(extension);
        // A late extension still gets its `fonts` hook.
        if self.hud.fonts_installed {
            self.hud.fonts_installed = false;
            self.refresh_project_fonts();
        }
        // And its debug panels, before the next scene load builds them again.
        #[cfg(feature = "debug-hud")]
        self.create_extension_debug_panels();
    }

    /// Give the procgen resolver a disk tier (#181). The resolver is
    /// rebuilt around it, and its generators move across, built-ins and the
    /// ones extensions registered alike (#195).
    #[cfg(feature = "procgen")]
    pub(super) fn use_procgen_disk(&mut self, disk: flint_procgen::DiskCacheConfig) {
        let registry = std::mem::take(self.core.procgen_resolver.registry_mut());
        self.core.procgen_resolver = flint_procgen::ProcGenResolver::with_registry(
            flint_procgen::ProcGenCacheConfig {
                disk: Some(disk),
                ..Default::default()
            },
            registry,
        );
    }

    /// A handle on the camera scripts' `world_to_screen` uses, for native
    /// functions an extension registers (ADR 0087). Take it before
    /// `add_extension` and move it into the extension, whose
    /// `register_script_api` closures clone it. Called from a script, it
    /// answers with the camera this frame renders with, as
    /// `world_to_screen` does; from `pre_update`, last frame's, like
    /// `ExtensionCtx::camera`.
    pub fn projector(&self) -> crate::Projector {
        self.core.script.projector()
    }

    /// Take the scene file's player settings: audio preload, input config,
    /// skybox, ambient and shading levers, camera, post-processing, living
    /// props and wooze. Every entry (`main.rs`, `flint play`, the web's
    /// `flint_web.rs`, Android) calls it after `new`, and a transition calls
    /// it for the scene it enters (#723). Each setting is replaced from the
    /// file, so a block the scene doesn't author means the defaults, never
    /// what the last scene had.
    pub fn apply_scene_settings(&mut self, scene_file: &flint_scene::SceneFile) {
        self.scene.preload_audio = scene_file.scene.preload_audio;
        self.scene.input_config = scene_file.scene.input_config.clone();

        // No shadows, no built-in lights (#627)
        let env = scene_file.environment.as_ref();
        self.scene.shadows = env.is_none_or(|e| e.casts_shadows());
        self.scene.default_lights = env.is_none_or(|e| e.uses_default_lights());

        // Skybox path, ambient and shading levers
        self.scene.skybox_path = env.and_then(|e| e.skybox.clone());
        self.scene.ambient = env.and_then(|e| {
            (e.ambient_sky.is_some() || e.ambient_ground.is_some()).then(|| {
                (
                    e.ambient_sky
                        .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_SKY),
                    e.ambient_ground
                        .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_GROUND),
                )
            })
        });
        self.scene.diffuse_wrap = env.and_then(|e| e.diffuse_wrap);
        self.scene.oren_nayar = env.and_then(|e| e.oren_nayar);
        self.scene.sheen = env.and_then(|e| {
            e.sheen_strength
                .map(|s| (e.sheen_color.unwrap_or([1.0; 3]), s))
        });

        self.scene.camera = scene_file.camera.clone();
        // User styles first: the post_process tables are read against the
        // registry, and a table for an unloaded style is dropped. The
        // scene's styles in place of the last scene's (#742): the game's
        // kept, and a user style only another scene could see gone
        let scope = self.asset_scope();
        let binding =
            flint_render::SceneBinding::over(scope, &self.core.styles).with_environment(env);
        for message in binding.style_messages() {
            tracing::warn!("{message}");
        }
        self.core.styles = binding.styles().clone();
        self.scene_binding = Some(binding);
        // Living props' tables, with the preset `[wooze] preset` names
        // filled in (it can set the shading boil in `[post_process]` too).
        // `flint_scene::load_scene` did this until #686; a host that has
        // already run `flint_procgen::settle_scene` loses nothing by it,
        // since a preset that has been filled in is no longer named. On a
        // copy of the tables: this takes the scene by reference.
        // A build without the `living` feature (#754) reads neither table.
        #[cfg(feature = "living")]
        {
            let mut settled = flint_scene::SceneFile::new(scene_file.scene.name.clone());
            settled.tables = scene_file.tables.clone();
            settled.post_process = scene_file.post_process.clone();
            let scene_path = std::path::PathBuf::from(&self.core.scene_path);
            if let Err(problem) = flint_procgen::settle_scene(&mut settled, &scene_path) {
                tracing::warn!("{problem}");
            }
            self.set_scene_post_process(settled.post_process.clone());
            let living_tables = flint_procgen::LivingTables::or_warn(&settled);
            self.scene.living = living_tables.living;
            self.scene.wooze = living_tables.wooze;
        }
        #[cfg(not(feature = "living"))]
        self.set_scene_post_process(scene_file.post_process.clone());
        self.scene.tables = scene_file.scene_tables();
    }

    /// Take a scene's `[post_process]` block, with the look it names
    /// (`look = "graphite"`, #356) laid under its own keys and `--look`
    /// (#435) over them. A look that can't be found is warned about and the
    /// block drawn without it. Call after the scene's user styles are
    /// loaded (a look may name one).
    pub fn set_scene_post_process(&mut self, def: Option<flint_scene::PostProcessDef>) {
        let scope = self.asset_scope();
        self.scene.post_process = super::scene_loading::scene_post_process_in(
            self.launch.look.as_deref(),
            &self.core.scene_path,
            &scope,
            def,
        );
        self.publish_look();
    }

    /// The look the scene draws with now: the one it names, or the one a
    /// script or F4 chose since (#356). `None` for the scene's keys alone.
    pub fn current_look(&self) -> Option<&str> {
        self.scene.post_process.as_ref()?.look.as_deref()
    }

    /// Tell scripts' `get_look()` the [current look](Self::current_look)
    /// (#497). Called where it changes and once a frame, before scripts, so
    /// a game that sets `scene.post_process` itself is heard too.
    pub(super) fn publish_look(&mut self) {
        let look = self
            .scene
            .post_process
            .as_ref()
            .and_then(|d| d.look.as_deref());
        self.core.script.set_current_look(look);
    }

    /// Tell scripts' `list_looks()` every look the scene can see (#497):
    /// its project's look files and the built-ins, as F4's picker lists
    /// them. At each scene's entry, before its scripts' `on_init`.
    pub(super) fn publish_looks(&mut self) {
        let names = flint_render::look::list_in(&self.kept_asset_scope())
            .into_iter()
            .map(|look| look.name)
            .collect();
        self.core.script.set_looks(names);
        self.publish_look();
    }

    /// Lay `look` under the scene's `[post_process]` keys in place of the
    /// one it has (`None` or `""`: no look, the scene's keys alone), and
    /// draw with it from the next frame (#356). Script post overrides keep
    /// stamping their fields over it, as they do over the scene's. A scene
    /// with no `[post_process]` block takes the look over the defaults.
    /// A cut: [`fade_to_look`](Self::fade_to_look) eases into it instead.
    pub fn set_look(&mut self, look: Option<&str>) -> Result<(), String> {
        self.fade_to_look(look, 0.0)
    }

    /// [`set_look`](Self::set_look) as a cross-fade over `seconds` of game
    /// time (#496): from the config drawn now to the one `look` resolves
    /// to, by `flint_render::look_fade` (numbers lerp, a style in one look
    /// alone fades by its blend, options switch at the midpoint), and
    /// `look_faded` fires to scripts at the end. 0 seconds, or no renderer
    /// yet, is the cut. The look is the current one at once
    /// ([`current_look`](Self::current_look)); a later look, faded or cut,
    /// starts from wherever this fade has reached.
    pub fn fade_to_look(&mut self, look: Option<&str>, seconds: f64) -> Result<(), String> {
        let def = self.look_def(look)?;
        self.scene.post_process = Some(def);
        // A fade's target is the look from its start, for get_look()
        self.publish_look();
        let config = self.scene_post_config();
        let now = self.core.clock.total_time;
        let running = self.style.look_fade.take();
        let device = self.render_context.as_ref().map(|c| &c.device);
        let device = device.or(self.windowless_gpu().map(|g| &g.device));
        if let (Some(renderer), Some(device)) = (self.scene_renderer.as_mut(), device) {
            // From the frame as it is now: a fade under way at this moment
            // of it, not at the last render's (#1157), as a capture lays it
            let mut from = renderer.post_process_config().clone();
            if let Some(running) = &running {
                running.at_time(now).apply(&mut from);
            }
            // Everything the new look switches on is built now, not on the
            // fade's first frame
            renderer.set_post_process_config(config.clone());
            renderer.ensure_style_resources(device);
            renderer.ensure_fxaa_resources(device);
            if seconds > 0.0 && seconds.is_finite() {
                renderer.set_post_process_config(from.clone());
                let fade = flint_render::look_fade::LookFade::new(
                    from,
                    config,
                    seconds as f32,
                    &self.core.styles,
                );
                self.style.look_fade = Some(super::state::LookFadeState {
                    fade,
                    look: look.map(str::to_string),
                    at: now,
                });
            }
        }
        #[cfg(feature = "debug-hud")]
        self.refresh_style_panels_after_look();
        Ok(())
    }

    /// The scene's `[post_process]` with `look` laid under its keys, as
    /// [`fade_to_look`](Self::fade_to_look) draws it. From the project the
    /// scene's entry found, and a preloaded look as read there (#1159): a
    /// cut asks the disk nothing for either. Asking again (the project's
    /// `flint.toml` and every folder up to it, then the look's search)
    /// stalled cuts 190-245 ms on a busy disk, preloaded or not.
    fn look_def(&self, look: Option<&str>) -> Result<flint_scene::PostProcessDef, String> {
        let base = match &self.scene.post_process {
            Some(def) => def.clone(),
            None => flint_scene::PostProcessDef::from_authored(toml::Table::new())
                .map_err(|e| e.to_string())?,
        };
        flint_render::look::resolve_read(
            &base,
            look,
            &self.kept_asset_scope(),
            &self.style.preloaded_looks,
        )
    }

    /// Build `look`'s styles on worker threads now (#1121), so the
    /// [`set_look`](Self::set_look) that shows it later costs its frame
    /// nothing: the first ASCII look stopped the game 1.3 s (Vulkan) to
    /// 1.7 s (DX12) while its 15 pipelines were built on that frame. The
    /// look isn't shown and nothing a script reads changes. Returns the
    /// styles started (none when every one is built or building, or there
    /// is no renderer yet). A look the search can't find is an error, as
    /// for `set_look`.
    pub fn prepare_look(&mut self, look: &str) -> Result<Vec<&'static str>, String> {
        let def = self.look_def(Some(look))?;
        let config = self.post_config_of(Some(&def));
        let device = self.render_context.as_ref().map(|c| &c.device);
        let device = device.or(self.windowless_gpu().map(|g| &g.device));
        let (Some(renderer), Some(device)) = (self.scene_renderer.as_mut(), device) else {
            return Ok(Vec::new());
        };
        let started = renderer.prepare_styles(device, &config);
        if !started.is_empty() {
            tracing::debug!(
                target: "player",
                "prepare_look {look}: building {} ahead",
                started.join(", ")
            );
        }
        Ok(started)
    }

    /// Build at the scene's entry, on worker threads, what its later
    /// frames would otherwise build on the frame that first shows it
    /// (#1159): the stage draws (blend, reveal, tear) its own styles'
    /// tables ask for, which no frame draws until a reveal lies between 0
    /// and 1 (Plaintext's tear stalled 92 ms on it); each look in
    /// `[post_process] preload_looks`, read once and kept for the cut
    /// ([`look_def`](Self::look_def)), with every stage draw, since a fade
    /// to a look blends its styles in; and each style in `preload_styles`
    /// switched on, with every stage draw, for a script that switches it
    /// on, tweens its reveal or tears it later (Plaintext's `matrix` and
    /// `tron`). Nothing is shown and nothing a script reads changes. A look
    /// the search can't find or a style the registry doesn't hold is
    /// warned about and skipped. Returns the styles started.
    pub(super) fn preload_scene_styles(
        &mut self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
    ) -> Vec<&'static str> {
        let own = self.scene_post_config();
        let mut started = renderer.prepare_styles(device, &own);
        self.style.preloaded_looks.clear();
        let Some(def) = self.scene.post_process.clone() else {
            return started;
        };
        let every: Vec<&'static str> = renderer.styles().names();
        let scope = self.kept_asset_scope();
        for look in &def.preload_looks {
            // Read once here: the cut to it reads nothing
            match flint_render::look::find_in(look, &scope) {
                Ok(found) => {
                    self.style.preloaded_looks.insert(look.clone(), found);
                }
                Err(e) => {
                    tracing::warn!("preload_looks: {e}");
                    continue;
                }
            }
            match self.look_def(Some(look)) {
                Ok(look_def) => {
                    let config = self.post_config_of(Some(&look_def));
                    started.extend(renderer.prepare_styles_with(device, &config, &every));
                }
                Err(e) => tracing::warn!("preload_looks: {e}"),
            }
        }
        if !def.preload_styles.is_empty() {
            let mut config = own;
            let mut named = Vec::new();
            for name in &def.preload_styles {
                match renderer.styles().find(name) {
                    Some(descriptor) => {
                        config.styles.set_enabled(descriptor.name, true);
                        named.push(descriptor.name);
                    }
                    None => tracing::warn!(
                        "preload_styles: no style named '{name}' ({})",
                        renderer.styles().names().join(", ")
                    ),
                }
            }
            started.extend(renderer.prepare_styles_with(device, &config, &named));
        }
        if !started.is_empty() {
            tracing::debug!(
                target: "player",
                "preloading {}",
                started.join(", ")
            );
        }
        started
    }

    /// Whether a look's cross-fade is under way (#496).
    pub fn look_fading(&self) -> bool {
        self.style.look_fade.is_some()
    }

    /// Lay a look's cross-fade on the renderer's config as it is at game
    /// time `now` (only what the two looks differ in): `now - at` seconds
    /// in, so the frame drawn at the moment a fade is asked is where it
    /// starts, and a capture at the same game time lays the same values
    /// (#1157). At its end, fire `look_faded` with `#{ look }` and drop it.
    /// Each render calls it once with the clock's `total_time`, before the
    /// scripts' overrides are stamped.
    pub(super) fn lay_look_fade(&mut self, now: f64) {
        let (Some(state), Some(renderer)) =
            (self.style.look_fade.as_ref(), self.scene_renderer.as_mut())
        else {
            return;
        };
        let fade = state.at_time(now);
        let mut config = renderer.post_process_config().clone();
        fade.apply(&mut config);
        renderer.set_post_process_config(config);
        if fade.done() {
            let look = state.look.clone();
            let mut data = toml::Table::new();
            data.insert("look".into(), look.unwrap_or_default().into());
            self.core.events.push(flint_runtime::GameEvent::Custom {
                name: super::style_commands::LOOK_FADED.into(),
                data: toml::Value::Table(data),
            });
            self.style.look_fade = None;
        }
    }

    /// Set the schema directories transitions and chunks load from, as a
    /// list the host settled itself: those that exist are kept, and a scene
    /// a transition leads to adds any `schemas/` beside it or one level up.
    /// A host that resolved them (`SchemaRegistry::resolve`) hands the
    /// answer to [`set_schemas`](Self::set_schemas) instead. The
    /// directories are read once, at the first transition or chunk.
    pub fn set_schema_paths(&mut self, paths: Vec<String>) {
        self.schema_dirs = flint_core::project::host_schema_dirs(&paths, None);
        self.schemas.forget();
    }

    /// Hand over the schema registry the first scene was loaded with, and
    /// the directories it was read from as the one resolver named them
    /// (`SchemaRegistry::resolve`'s two answers). The player keeps the
    /// registry (ADR 0115, #683): chunks use it, a transition reads the
    /// disk again only when the next scene's directories differ, and a read
    /// that fails keeps this one, so a scene never loads with none.
    pub fn set_schemas(
        &mut self,
        registry: flint_schema::SchemaRegistry,
        dirs: flint_core::project::SchemaDirs,
    ) {
        self.schemas.set(registry, &dirs.dirs);
        self.schema_dirs = dirs;
    }

    /// Advance the game exactly `step` seconds a frame, whatever the wall
    /// time (`--frame-step`, the web's `?frame_step=`; none or 0 is the
    /// wall clock), and seed scripts' `random()` with `seed`. A stepped run
    /// is meant to repeat, so with no seed it takes the one `flint render
    /// --scripts` uses (#635); neither leaves the clock-started sequence.
    pub fn step_frames(&mut self, step: Option<f64>, seed: Option<u64>) {
        self.core.clock.frame_step = step.filter(|s| *s > 0.0);
        if let Some(seed) = seed.or(self
            .core
            .clock
            .frame_step
            .map(|_| super::headless_game::SCRIPT_SEED))
        {
            self.core.script.seed_random(seed);
        }
    }

    /// Set the schema directories as the one resolver named them (ADR
    /// 0114), with where the list came from: `--schemas` flags stay as they
    /// are across transitions, a manifest's list is read again for the new
    /// scene's project, and a discovered one grows as it always did.
    /// With no registry handed over ([`set_schemas`](Self::set_schemas)),
    /// they are read once, at the first transition or chunk.
    pub fn set_schema_dirs(&mut self, dirs: flint_core::project::SchemaDirs) {
        self.schema_dirs = dirs;
        self.schemas.forget();
    }

    /// Register a function that reads gamepad trigger/right-stick axes from the
    /// Android JNI bridge. Called each frame in `poll_gamepad_events()`.
    /// Returns [left_trigger, right_trigger, right_stick_x, right_stick_y].
    #[cfg(target_os = "android")]
    pub fn set_android_axis_reader(&mut self, reader: fn() -> [f32; 4]) {
        self.android_axis_reader = Some(reader);
    }

    /// The post-processing config a scene starts with: its `[post_process]`
    /// with the `--style` words written over it (#328). A scene with no
    /// block starts from the defaults, the config a new renderer has, not
    /// from what the last scene left (#723). Read against the scene's own
    /// registry (#742), so a user style only an earlier scene could see is
    /// an unknown style here, as it is when the scene loads first (#426);
    /// the scene's own tables keep it, so a file the scene can see that
    /// takes the name over later brings it back.
    pub fn scene_post_config(&self) -> flint_render::PostProcessConfig {
        self.post_config_of(self.scene.post_process.as_ref())
    }

    /// [`scene_post_config`](Self::scene_post_config) of `def` in place
    /// of the scene's: a look about to be shown (#1121).
    fn post_config_of(
        &self,
        def: Option<&flint_scene::PostProcessDef>,
    ) -> flint_render::PostProcessConfig {
        let styles = &self.core.styles;
        let mut config = def
            .map(|def| scene_loading::post_process_config_from_def(def, styles))
            .unwrap_or_default();
        for e in
            flint_render::stylize::apply_style_specs(&mut config, &self.launch.style_specs, styles)
        {
            tracing::warn!("{e}");
        }
        if let Some(rate) = self.launch.motion_hold {
            config.motion_hold = rate.max(0.0);
        }
        config
    }

    /// Apply scene-level camera configuration from `CameraDef`
    pub(super) fn apply_camera_def(&mut self) {
        if let Some(cam) = &self.scene.camera {
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
        use flint_core::load_report::{self, Stage};
        let window = load_report::timed(Stage::Window, String::new, || {
            self.create_window(event_loop)
        })?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let render_context = load_report::timed(Stage::Device, String::new, || {
                pollster::block_on(RenderContext::with_present(
                    window.clone(),
                    crate::present_args::present_options(&self.launch),
                ))
            })
            .context("Failed to initialize render context")?;
            self.finish_initialize(window, render_context)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.request_render_context(window);
            Ok(())
        }
    }

    /// Everything after the device: renderer, egui, quality, then the first
    /// scene through the one bring-up a transition uses too
    /// (`enter_scene`, #723).
    pub(super) fn finish_initialize(
        &mut self,
        window: Arc<Window>,
        render_context: RenderContext,
    ) -> Result<()> {
        use flint_core::load_report::{self, Stage};
        self.camera.aspect = render_context.aspect_ratio();
        let mut scene_renderer = load_report::timed(Stage::Renderer, String::new, || {
            SceneRenderer::new(
                &render_context,
                flint_render::RendererConfig {
                    sample_count: self.launch.msaa_sample_count,
                    ..Default::default()
                },
            )
        });
        // Parts skipped or repeated by label (#1317)
        if let Some(knobs) = &self.launch.pass_knobs {
            tracing::info!(target: "player", "pass knobs: {}", knobs.describe());
            scene_renderer.set_pass_knobs(Some(knobs.clone()));
        }

        // Gamepad backend and the egui overlay
        load_report::timed(Stage::Hud, String::new, || {
            self.init_egui(&window, &render_context)
        });

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);
        self.update_vsync_pacing();
        // Quality settings (FlintKart#55; FLINT_SHADOWS, FLINT_SHADOW_RES and
        // the rest) before any script's on_init, so get_quality() and
        // gpu_info() answer from the first frame.
        self.init_quality();

        // Which project the scene is in, and the procgen disk tier (#181),
        // set up once, before the first scene's assets are resolved.
        self.refresh_project();
        #[cfg(feature = "procgen")]
        self.init_procgen_disk_cache();

        self.enter_scene(super::scene_entry::SceneEntry::Start)
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<Arc<Window>> {
        let window_attrs = Window::default_attributes().with_title("Flint Player");
        // In a browser the page's canvas is the window, sized by its CSS.
        #[cfg(target_arch = "wasm32")]
        let window_attrs = super::web_startup::canvas_attributes(window_attrs);
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        let window_attrs = window_attrs.with_inner_size(PhysicalSize::new(
            self.launch.window_size.0.max(1),
            self.launch.window_size.1.max(1),
        ));
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        let window_attrs = self.launch.window_options.apply(window_attrs);

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create player window")?,
        );

        if self.launch.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        self.window = Some(window.clone());

        Ok(window)
    }

    /// The procgen disk tier (#181), once: a resolver made with it is kept
    /// for the run, transitions included.
    #[cfg(feature = "procgen")]
    fn init_procgen_disk_cache(&mut self) {
        if self.core.procgen_resolver.disk_stats().is_some() {
            return;
        }
        let scene_dir = self.scene_dir();
        if let Some(disk) = self
            .launch
            .procgen_disk_cache
            .then(|| procgen_disk_config(&self.core.project, &scene_dir))
            .flatten()
        {
            tracing::debug!(target: "load", "[procgen] disk cache: {}", disk.dir.display());
            self.use_procgen_disk(disk);
        }
    }

    fn init_egui(&mut self, window: &Arc<Window>, render_context: &RenderContext) {
        // Initialize gamepad backend (best-effort).
        self.gilrs = Gilrs::new().ok();

        // Initialize egui for HUD overlay. egui assumes textures of at most
        // 2048 px a side until told otherwise, and panics on a wider one
        // (Float's 2713 px title). Give it the device's limit, both for the
        // passes egui-winit feeds it and now, since UI sprites load before
        // the first pass.
        let max_texture_side = render_context.device.limits().max_texture_dimension_2d as usize;
        self.hud
            .egui_ctx
            .input_mut(|i| i.max_texture_side = max_texture_side);
        let egui_winit = egui_winit::State::new(
            self.hud.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(max_texture_side),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &render_context.device,
            render_context.hud_format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                dithering: false,
                ..Default::default()
            },
        );
        self.hud.egui_winit = Some(egui_winit);
        self.hud.egui_renderer = Some(egui_renderer);
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
            self.core
                .script
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
            self.core.script.set_terrain_height_fn(None);
        }
    }

    pub(super) fn configure_input_bindings(&mut self) -> Result<()> {
        self.input
            .load_bindings(InputConfig::built_in_defaults())
            .context("failed to load built-in input defaults")?;

        let mut paths = resolve_input_paths(
            &self.core.project,
            Path::new(&self.core.scene_path),
            self.scene.input_config.as_deref(),
            self.input_layers.config_override.as_deref(),
        );
        // What the scene's own layers bind, for the controls line (#716)
        self.input_layers.named_actions.clear();

        if let Some(path) = &paths.game_default {
            if vfs::exists(path) {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load game input config '{}'", path.display())
                })?;
                self.input_layers
                    .named_actions
                    .extend(cfg.actions.keys().cloned());
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge game input config")?;
            }
        }

        // Now the game default has set the game id, find the player's own
        // rebinds: they're saved under it.
        paths.user_override = resolve_user_override(
            &self.core.project,
            Path::new(&self.core.scene_path),
            &self.input.config().game_id,
        );
        // The player's copy (crate::user_files, localStorage on the web), else
        // one the game ships in its vfs bundle.
        if let Some(path) = &paths.user_override {
            let cfg = read_user_override_file(path).with_context(|| {
                format!("failed to load user input config '{}'", path.display())
            })?;
            if let Some(cfg) = cfg {
                self.input_layers.user_override = cfg.clone();
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
                self.input_layers
                    .named_actions
                    .extend(cfg.actions.keys().cloned());
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge CLI input config")?;
            }
        }

        if self.input_layers.user_override.version == 0 {
            self.input_layers.user_override.version = 1;
        }
        if self.input_layers.user_override.game_id.trim().is_empty() {
            self.input_layers.user_override.game_id = self.input.config().game_id.clone();
        }

        self.input_layers.config_paths = Some(paths);
        Ok(())
    }

    /// The project the current scene belongs to (ADR 0114).
    pub fn project(&self) -> &flint_core::project::Project {
        &self.core.project
    }

    /// The current scene's directory: where every asset search starts.
    pub(super) fn scene_dir(&self) -> std::path::PathBuf {
        Path::new(&self.core.scene_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    /// The current scene's project and directory, for the loaders that
    /// search from them (looks, user styles, fonts, the renderer's styles).
    /// The project is asked for again first, so this is right before the
    /// renderer exists and straight after a transition.
    pub(super) fn asset_scope(&mut self) -> flint_core::project::AssetScope {
        self.refresh_project();
        self.kept_asset_scope()
    }

    /// [`asset_scope`](Self::asset_scope) with the project as last found,
    /// for a poll.
    pub(super) fn kept_asset_scope(&self) -> flint_core::project::AssetScope {
        flint_core::project::AssetScope::new(self.core.project.clone(), self.scene_dir())
    }

    /// Ask again which project the current scene belongs to, and take the
    /// answer when it differs: at the first load, and after a transition,
    /// which may cross into another project's root (or a `flint.toml` may
    /// have changed under a long session). `--project` pins one answer.
    pub(crate) fn refresh_project(&mut self) {
        let next = flint_core::project::Project::for_scene(Path::new(&self.core.scene_path));
        if next != self.core.project {
            tracing::info!(
                "project root: {} ({})",
                next.root.display(),
                next.found.describe()
            );
            self.core.project = next;
        }
        self.declare_audio_buses();
    }

    /// The mixer's buses from the project's `flint.toml` `[audio.buses]`
    /// (#730). The engine applies a table once, so this is free at every
    /// scene of one project and keeps the volumes set since; a project
    /// with another table adds its buses. A manifest that doesn't read is
    /// said and the buses stay as they are.
    pub(crate) fn declare_audio_buses(&mut self) {
        match flint_audio::Buses::for_project(&self.core.project) {
            Ok(buses) => self.audio.engine.set_buses(&buses),
            Err(e) => tracing::error!(target: "load", "audio buses: {e}"),
        }
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
            &self.core.particles,
            renderer,
            &context.device,
            &context.queue,
            &self.core.project,
            &self.core.scene_path,
        );
    }

    /// (Re)load the fonts the scene sees (`fonts/`, by the project's one
    /// asset search) into egui when they are other files than the last
    /// scan's (initial load and scene transitions). Extensions' `fonts`
    /// hooks edit the definitions before they're installed.
    pub(super) fn refresh_project_fonts(&mut self) {
        let scope = self.asset_scope();
        let root = super::fonts::ProjectFonts::find(&scope.project, &scope.scene_dir);
        if self.hud.fonts_installed && self.hud.fonts_root == root {
            return; // the same files as the last scan
        }
        let mut defs = super::fonts::font_definitions(&root);
        for ext in self.extensions.iter_mut() {
            ext.fonts(&mut defs);
        }
        self.hud.fonts = super::fonts::script_families(&defs);
        self.hud.egui_ctx.set_fonts(defs);
        self.hud.fonts_warned.clear();
        self.hud.fonts_root = root;
        self.hud.fonts_installed = true;
    }

    /// Give scripts a `measure_text` that lays text out with the same font
    /// stack the HUD draws with. Before egui's first pass (fonts not built
    /// yet) and for families not installed in the *active* definitions,
    /// the closure degrades gracefully instead of panicking.
    pub(super) fn install_text_measurer(&mut self) {
        let measurer = super::fonts::text_measurer(
            self.hud.egui_ctx.clone(),
            Some(self.hud.egui_fonts_ready.clone()),
        );
        self.core.script.set_text_measurer(Some(measurer));
    }

    /// Load a sprite texture for UI rendering. Called lazily when a draw_sprite
    /// command references a name not yet in ui_textures.
    pub fn load_ui_texture(&mut self, name: &str) -> bool {
        if self.hud.textures.contains_key(name) {
            return true;
        }

        // The project's one search (ADR 0114): `sprites/<name>` then `<name>`
        // beside the scene, then the same beside each directory up to the
        // project root, then in the mounted engine.
        let scene_dir = Path::new(&self.core.scene_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        use flint_core::project::AssetKind;
        let problem = match self
            .core
            .project
            .resolve(AssetKind::Sprites, name, scene_dir)
        {
            Ok(path) => match flint_render::open_image(&path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    // Premultiplied in display values, as the HUD blends
                    // (#958)
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    let tex_handle = self.hud.egui_ctx.load_texture(
                        name,
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.hud.textures.insert(name.to_string(), tex_handle);
                    tracing::debug!(target: "load", kind = "ui_sprite", "Loaded UI sprite: {}", name);
                    return true;
                }
                Err(e) => format!("UI sprite '{name}': {}: {e}", path.display()),
            },
            Err(missing) => missing.to_string(),
        };
        // A draw command asks again every frame: say it once a scene folder.
        let key = format!("{}|{name}", scene_dir.display());
        if self.hud.sprites_warned.insert(key) {
            tracing::warn!("{problem}");
        }
        false
    }

    /// Pre-scan draw commands and load any sprite textures that haven't been loaded yet
    pub(super) fn load_pending_sprites(&mut self) {
        let sprite_names: Vec<String> = self
            .hud
            .draw_commands
            .iter()
            .filter_map(|item| {
                if let DrawCommand::Sprite { name, .. } = &item.cmd {
                    if !self.hud.textures.contains_key(name.as_str()) {
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
/// `FLINT_PROCGEN_CACHE=<dir>` if set (`0`, `off`, `false`, `no`: no disk tier),
/// else `.flint/procgen-cache/` at the scene's project root
/// (`Project::root_for`, ADR 0114), so every scene of a project shares one
/// cache and the `.flint/` it makes marks the root that was already in use.
#[cfg(feature = "procgen")]
fn procgen_disk_config(
    project: &flint_core::project::Project,
    scene_dir: &Path,
) -> Option<flint_procgen::DiskCacheConfig> {
    match flint_core::env::PROCGEN_CACHE.get() {
        Some(flint_core::env::Value::Off) => return None,
        Some(flint_core::env::Value::Path(dir)) => {
            return Some(flint_procgen::DiskCacheConfig::new(dir))
        }
        _ => {}
    }
    Some(flint_procgen::DiskCacheConfig::in_project(
        &project.root_for(scene_dir),
    ))
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
    fn without_words_or_a_block_a_scene_draws_the_defaults() {
        // The config a new renderer has, so a scene with no block never
        // keeps the last scene's (#723)
        assert_eq!(
            format!("{:?}", app().scene_post_config()),
            format!("{:?}", flint_render::PostProcessConfig::default())
        );
    }

    #[test]
    fn style_words_win_over_every_scene_they_meet() {
        let mut app = app();
        app.launch.style_specs = vec!["ascii:cell=12".into()];

        // A scene with no [post_process] still gets the style
        let config = app.scene_post_config();
        assert!(config.styles.is_enabled("ascii"));
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));

        // A scene that authors the same style, and keeps its own bloom:
        // the word wins on the style and leaves the rest alone. This is
        // what a transition re-reads (#328)
        app.scene.post_process = Some(scene(
            "[post_process]\nbloom_intensity = 0.3\n\
             [post_process.styles.ascii]\ncell = 6\nramp = \"blocks\"\n",
        ));
        let config = app.scene_post_config();
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));
        assert_eq!(config.styles.option("ascii", "ramp"), Some("blocks"));
        assert!((config.bloom_intensity - 0.3).abs() < 1e-6);

        // ascii:off turns off a style the scene has on
        app.launch.style_specs = vec!["ascii:off".into()];
        assert!(!app.scene_post_config().styles.is_enabled("ascii"));
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
        let config = app.scene_post_config();
        assert!(config.styles.is_enabled("pencil"), "graphite's pencil");
        assert!(
            (config.bloom_intensity - 0.3).abs() < 1e-6,
            "the scene's key"
        );

        app.set_look(Some("matrix")).unwrap();
        assert_eq!(app.current_look(), Some("matrix"));
        let config = app.scene_post_config();
        assert!(config.styles.is_enabled("ascii"), "matrix is ASCII");
        assert!(!config.styles.is_enabled("pencil"), "graphite is gone");
        assert!((config.bloom_intensity - 0.3).abs() < 1e-6);

        app.set_look(None).unwrap();
        assert_eq!(app.current_look(), None);
        let config = app.scene_post_config();
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
        assert_ne!(
            format!("{:?}", app.scene_post_config()),
            format!("{:?}", flint_render::PostProcessConfig::default())
        );
    }
}
