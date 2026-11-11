//! The one scene bring-up (#723). Startup and every scene transition bring
//! the current scene up through [`PlayerApp::enter_scene`], so what the
//! first scene gets, the tenth gets too.
//!
//! The scene's settings were taken from its file by `apply_scene_settings`
//! first. Each subsystem here is set from them as a whole: a block the
//! scene doesn't author means the defaults (no skybox, the renderer's
//! default post-processing, the project's input config, a fresh camera),
//! never what the last scene left. What tears the last scene down is
//! `teardown_current_scene` in `transition.rs`.

use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_scripts_from_world, load_sequences_from_world, load_sprite_animations_from_world,
    load_terrain_from_world_inner, register_node_animation_data, register_skeletal_data,
    LoadedGrassInfo,
};
use super::PlayerApp;
use anyhow::{Context, Result};
use flint_core::vfs;
use flint_render::model_loader;
use flint_render::{Camera, SceneRenderer};
use flint_runtime::RuntimeSystem;

/// How the scene is reached. The bring-up is the same; these say what
/// happens once per run and what only a scene change has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SceneEntry {
    /// The first scene: the window, renderer and egui were just made. The
    /// text measurer is installed and the save file read, once.
    Start,
    /// A transition: the last scene was torn down. Scripts hear
    /// `on_scene_enter` after their `on_init`.
    Transition,
}

impl PlayerApp {
    /// Bring the current scene (`self.world`, `self.scene_path` and the
    /// settings `apply_scene_settings` took) up: input bindings, camera,
    /// fonts, the renderer's share (models, lights, skybox, post-processing,
    /// terrain), the debug panels, then physics, audio, the music session,
    /// animation, particles and scripts, in that order.
    ///
    /// An error is a startup failure (physics would not initialize); at a
    /// transition the same failure is logged and the scene plays on.
    pub(super) fn enter_scene(&mut self, entry: SceneEntry) -> Result<()> {
        self.world.rebuild_component_index();
        // Which project the scene is in, before anything is searched for
        self.refresh_project();

        // The scene's `[scene] input_config`, else the project's, under the
        // player's own rebinds and the CLI's
        self.configure_input_bindings()
            .unwrap_or_else(|e| tracing::warn!("Input config load error: {e:#}"));

        // A fresh camera, then the scene's `[camera]` over it
        self.reset_camera();

        // Project fonts may differ when the scene lives in another project
        self.refresh_project_fonts();
        if entry == SceneEntry::Start {
            self.install_text_measurer();
        }

        let grass = self.stage_scene_in_renderer();
        #[cfg(feature = "debug-hud")]
        self.create_scene_debug_panels(grass);
        #[cfg(not(feature = "debug-hud"))]
        let _ = grass;
        self.apply_camera_tuning();
        // The terrain height callback for scripts
        self.update_terrain_height_fn();

        if entry == SceneEntry::Start {
            // The saved store (where: persist.rs) before any script's
            // on_init, so persist_get sees last session's values. After the
            // input config, whose game_id names the save folder of a game
            // with no `[project] id`. A transition keeps the store.
            self.load_persistent_store();
        }

        self.start_scene_systems(entry)?;

        // On Android the cursor always counts as captured, so touch input
        // flows without a click-to-capture gate; elsewhere a first-person
        // player captures it.
        #[cfg(target_os = "android")]
        {
            self.cursor_captured = true;
        }
        #[cfg(not(target_os = "android"))]
        if self.physics.has_player_entity() && !self.cursor_captured {
            self.capture_cursor();
        }
        Ok(())
    }

    /// The camera a scene starts with: the defaults at the window's aspect,
    /// with the scene's `[camera]` over them. A scene with no `[camera]`
    /// does not inherit an orthographic camera or a field of view from the
    /// last one.
    fn reset_camera(&mut self) {
        let aspect = self.camera.aspect;
        self.camera = Camera::new();
        self.camera.aspect = aspect;
        self.camera.fov = 70.0; // Slightly wider FOV for first-person
        self.apply_camera_def();
    }

    /// Run [`stage_scene`](Self::stage_scene) on the renderer, when there
    /// is one (a headless test has none). The renderer and its context are
    /// taken off `self` for the call and put back.
    fn stage_scene_in_renderer(&mut self) -> Option<LoadedGrassInfo> {
        let context = self.render_context.take();
        let mut renderer = self.scene_renderer.take();
        let gpu = context.as_ref().map(|c| (&c.device, &c.queue));
        #[cfg(test)]
        let gpu = gpu.or(self.test_gpu.map(|g| (&g.device, &g.queue)));
        let grass = match (renderer.as_mut(), gpu) {
            (Some(renderer), Some((device, queue))) => self.stage_scene(renderer, device, queue),
            _ => None,
        };
        self.render_context = context;
        self.scene_renderer = renderer;
        grass
    }

    /// The renderer's share of a scene: procgen and models, splines, the
    /// lighting levers, the skybox, post-processing and terrain, each set
    /// from the scene's settings. Returns the grass the terrain loaded, for
    /// its debug panel.
    pub(super) fn stage_scene(
        &mut self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<LoadedGrassInfo> {
        // The renderer's styles find their palettes, glyph sets and papers
        // from the scene's project and folder (ADR 0114)
        renderer.set_asset_scope(self.kept_asset_scope());

        let config = build_model_load_config(
            &self.project,
            &self.scene_path,
            &self.world,
            self.catalog.as_ref(),
            self.content_store.as_ref(),
        );

        // Living props and the wooze field, their clocks started over
        self.procgen_resolver
            .set_living_defaults(super::living_props::defaults_of(self.scene_living.as_ref()));
        self.wooze = super::living_props::field_of(self.scene_wooze.as_ref());
        self.wooze_time = 0.0;
        self.shallow_wooze.clear();
        renderer.set_shallow_wooze(std::iter::empty());
        super::shallow_wooze::warn_without_a_field(&self.world, self.wooze.as_ref());
        self.boil_clock = Default::default();
        self.grass_wooze = Default::default();
        self.living_tune.for_new_scene();

        // Procgen before the model loader: catalog > procgen > file (#32).
        super::procgen_assets::resolve_scene(
            &self.scene_path,
            &mut self.world,
            &mut self.procgen_resolver,
            self.procgen_mode,
            super::procgen_assets::GpuSink {
                renderer,
                device,
                queue,
                config: &config,
            },
        );

        let load_result =
            model_loader::load_models_from_world(&mut self.world, renderer, device, queue, &config);
        register_skeletal_data(&load_result, &mut self.animation);
        register_node_animation_data(&load_result, &mut self.animation);
        self.skeletal_entity_assets = load_result.skinned_entities;
        renderer.update_from_world(&self.world, device);

        // Procedural geometry from spline + spline_mesh entities
        crate::spline_gen::load_splines(
            &self.scene_path,
            &mut self.world,
            renderer,
            Some(&mut self.physics),
            device,
            queue,
        );
        renderer.update_from_world(&self.world, device);

        // The scene's first frame has no past: styles that keep history
        // (ASCII's temporal stability, #277) start over, and its first
        // camera is no cut from the last scene's
        renderer.reset_style_history();
        self.camera_cut.forget();

        // Lighting: the built-in ambient and levers, then what the scene
        // authors (absent = renderer default)
        renderer.reset_ambient();
        renderer.set_scene_shadows(self.scene_shadows);
        renderer.set_default_lights(self.scene_default_lights);
        if let Some((sky, ground)) = self.scene_ambient {
            renderer.set_ambient(sky, ground);
        }
        if let Some(wrap) = self.scene_diffuse_wrap {
            renderer.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = self.scene_oren_nayar {
            renderer.set_oren_nayar(oren);
        }
        if let Some((color, strength)) = self.scene_sheen {
            renderer.set_sheen(color, strength);
        }

        // Skybox: none unless the scene names one. A path found by the
        // project's one search (`AssetKind::Files`); a miss is warned about
        // with every path tried
        renderer.clear_skybox();
        if let Some(skybox_rel) = &self.skybox_path {
            let scene_dir = self.scene_dir();
            let skybox_path = self
                .project
                .resolve(
                    flint_core::project::AssetKind::Files,
                    skybox_rel,
                    &scene_dir,
                )
                .unwrap_or_else(|not_found| {
                    tracing::warn!("skybox: {not_found}");
                    scene_dir.join(skybox_rel)
                });
            if vfs::exists(&skybox_path) {
                renderer.load_skybox(device, queue, &skybox_path);
            }
        }

        // Post-processing: the scene's `[post_process]` with --style over
        // it, or the defaults, replacing whatever was set (scripts' sticky
        // overrides included)
        renderer.set_post_process_config(self.scene_post_config());
        renderer.ensure_style_resources(device);
        renderer.ensure_fxaa_resources(device);

        // Terrain, its colliders and grass
        let mut grass = None;
        load_terrain_from_world_inner(
            &self.world,
            &self.scene_path,
            device,
            queue,
            renderer,
            &mut self.physics,
            &mut self.terrain,
            &mut grass,
        );
        super::shallow_wooze::warn_grass_without_a_field(
            renderer.grass_config(),
            self.wooze.as_ref(),
        );
        grass
    }

    /// The scene's debug panels. The last scene's go (grass, ocean,
    /// particles, styles...); Rendering & Effects (F4) and the game's own
    /// belong to the player and are kept, open or closed as they were.
    /// Before #268 a transition dropped them and never made them again.
    #[cfg(feature = "debug-hud")]
    fn create_scene_debug_panels(&mut self, grass: Option<LoadedGrassInfo>) {
        self.drop_scene_debug_panels();
        if let Some(info) = grass {
            let panel = flint_debug_ui::GrassDebugPanel::new(
                info.config,
                std::path::PathBuf::from(&self.scene_path),
                info.terrain_entity_name,
            )
            .with_component(info.component);
            self.debug_panels.push(Box::new(panel));
        }
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

    /// Physics, audio, the music session, animation, particles and scripts,
    /// in that order: audio before the scene-declared music session, the
    /// session before script init so a conducted context exists from the
    /// scripts' first frame (F3/F4).
    fn start_scene_systems(&mut self, entry: SceneEntry) -> Result<()> {
        let physics = self
            .physics
            .initialize(&mut self.world)
            .context("Failed to initialize physics");
        match (physics, entry) {
            (Err(e), SceneEntry::Start) => return Err(e),
            (Err(e), SceneEntry::Transition) => tracing::warn!("{e:#}"),
            (Ok(()), _) => {}
        }

        load_audio_from_world(
            &self.world,
            &mut self.audio,
            &self.project,
            &self.scene_path,
            self.scene_preload_audio,
            // In the browser the audio may already be here, and decoding it
            // all in one frame is the stall #124 removed: queue it. The
            // desktop decodes up front, as a load can afford.
            cfg!(target_arch = "wasm32"),
        );
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        // The scene-declared music session (F3) on the freshly initialized
        // audio. Music to music: the fresh gilrs the teardown made is
        // dropped again by the new session's handoff.
        self.start_music_session();

        load_animations_from_world(&self.project, &self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.project, &self.scene_path, &mut self.animation);
        load_sequences_from_world(&self.project, &self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        // `particles/*.particles.toml` registered first, so `particle_effect`
        // components resolve on the first sync
        self.refresh_project();
        flint_particles::load_particle_effects_from_world(
            &self.project,
            &self.scene_path,
            &mut self.particles,
        );
        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        self.load_particle_textures();

        // Scripts: on_init with the state machine and the persistent store
        // in reach, then, at a transition, on_scene_enter
        load_scripts_from_world(&self.project, &self.scene_path, &mut self.script);
        self.script
            .set_current_scene(&self.scene_path, &self.project);
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script
                .initialize(&mut self.world)
                .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));
            if entry == SceneEntry::Transition {
                self.script.call_scene_enters(&mut self.world);
            }
        }
        Ok(())
    }
}
