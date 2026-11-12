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
    /// Bring the current scene (`self.core.world`, `self.core.scene_path` and the
    /// settings `apply_scene_settings` took) up: input bindings, camera,
    /// fonts, the renderer's share (models, lights, skybox, post-processing,
    /// terrain), the debug panels, then physics, audio,
    /// animation, particles and scripts, in that order.
    ///
    /// An error is a startup failure (physics would not initialize); at a
    /// transition the same failure is logged and the scene plays on.
    pub(super) fn enter_scene(&mut self, entry: SceneEntry) -> Result<()> {
        self.core.world.rebuild_component_index();
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
        // The subsystems this scene uses, each told it is up
        self.subsystems_enter_scene();

        // On Android the cursor always counts as captured, so touch input
        // flows without a click-to-capture gate; elsewhere a first-person
        // player captures it.
        #[cfg(target_os = "android")]
        {
            self.focus.cursor_captured = true;
        }
        #[cfg(not(target_os = "android"))]
        if self.core.physics.has_player_entity() && !self.focus.cursor_captured {
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
            &self.core.project,
            &self.core.scene_path,
            &self.core.world,
            self.assets.catalog.as_ref(),
            self.assets.content_store.as_ref(),
        );

        // What the scene says about living props, for the props procgen
        // adds (their clocks and field are the living subsystem's)
        #[cfg(feature = "living")]
        self.core
            .procgen_resolver
            .set_living_defaults(super::living_props::defaults_of(self.scene.living.as_ref()));

        // Procgen before the model loader: catalog > procgen > file (#32).
        // A build without it (#754) says what it won't make.
        #[cfg(not(feature = "procgen"))]
        if let Some(line) = super::missing_features::procgen_line(
            &self.core.world,
            &self.core.project,
            &self.core.scene_path,
        ) {
            println!("{line}");
        }
        #[cfg(feature = "procgen")]
        super::procgen_assets::resolve_scene(
            &self.core.scene_path,
            &mut self.core.world,
            &mut self.core.procgen_resolver,
            self.launch.procgen_mode,
            super::procgen_assets::GpuSink {
                renderer,
                device,
                queue,
                config: &config,
            },
        );

        let load_result = model_loader::load_models_from_world(
            &mut self.core.world,
            renderer,
            device,
            queue,
            &config,
        );
        register_skeletal_data(&load_result, &mut self.core.animation);
        register_node_animation_data(&load_result, &mut self.core.animation);
        self.skeletal_entity_assets = load_result.skinned_entities;
        renderer.update_from_world(&self.core.world, device);

        // Procedural geometry from spline + spline_mesh entities
        crate::spline_gen::load_splines(
            &self.core.scene_path,
            &mut self.core.world,
            renderer,
            Some(&mut self.core.physics),
            device,
            queue,
        );
        renderer.update_from_world(&self.core.world, device);

        // The scene's first frame has no past: styles that keep history
        // (ASCII's temporal stability, #277) start over, and its first
        // camera is no cut from the last scene's
        renderer.reset_style_history();
        self.style.camera_cut.forget();

        // Lighting: the built-in ambient and levers, then what the scene
        // authors (absent = renderer default)
        renderer.reset_ambient();
        renderer.set_scene_shadows(self.scene.shadows);
        renderer.set_default_lights(self.scene.default_lights);
        if let Some((sky, ground)) = self.scene.ambient {
            renderer.set_ambient(sky, ground);
        }
        if let Some(wrap) = self.scene.diffuse_wrap {
            renderer.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = self.scene.oren_nayar {
            renderer.set_oren_nayar(oren);
        }
        if let Some((color, strength)) = self.scene.sheen {
            renderer.set_sheen(color, strength);
        }

        // Skybox: none unless the scene names one. A path found by the
        // project's one search (`AssetKind::Files`); a miss is warned about
        // with every path tried
        renderer.clear_skybox();
        if let Some(skybox_rel) = &self.scene.skybox_path {
            let scene_dir = self.scene_dir();
            let skybox_path = self
                .core
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
            &self.core.world,
            &self.core.scene_path,
            device,
            queue,
            renderer,
            &mut self.core.physics,
            &mut self.terrain,
            &mut grass,
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
                std::path::PathBuf::from(&self.core.scene_path),
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
        #[cfg(feature = "living")]
        self.create_living_debug_panel();
        self.create_styles_debug_panel();
        // Rendering & Effects (ADR 0053): unconditional, F4 summons it
        self.create_render_debug_panel();
        // The game's own (ADR 0107), after the engine's.
        self.create_extension_debug_panels();
    }

    /// Physics, audio, animation, particles and scripts, in that order. The
    /// scene's music is a subsystem, started on this audio when the
    /// subsystems hear the scene loaded, before the scripts' first frame.
    fn start_scene_systems(&mut self, entry: SceneEntry) -> Result<()> {
        let physics = self
            .core
            .physics
            .initialize(&mut self.core.world)
            .context("Failed to initialize physics");
        match (physics, entry) {
            (Err(e), SceneEntry::Start) => return Err(e),
            (Err(e), SceneEntry::Transition) => tracing::warn!("{e:#}"),
            (Ok(()), _) => {}
        }

        load_audio_from_world(
            &self.core.world,
            &mut self.audio,
            &self.core.project,
            &self.core.scene_path,
            self.scene.preload_audio,
            // In the browser the audio may already be here, and decoding it
            // all in one frame is the stall #124 removed: queue it. The
            // desktop decodes up front, as a load can afford.
            cfg!(target_arch = "wasm32"),
        );
        self.audio
            .initialize(&mut self.core.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        load_animations_from_world(
            &self.core.project,
            &self.core.scene_path,
            &mut self.core.animation,
        );
        load_sprite_animations_from_world(
            &self.core.project,
            &self.core.scene_path,
            &mut self.core.animation,
        );
        load_sequences_from_world(
            &self.core.project,
            &self.core.scene_path,
            &mut self.core.animation,
        );
        self.core
            .animation
            .initialize(&mut self.core.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        // `particles/*.particles.toml` registered first, so `particle_effect`
        // components resolve on the first sync
        self.refresh_project();
        flint_particles::load_particle_effects_from_world(
            &self.core.project,
            &self.core.scene_path,
            &mut self.core.particles,
        );
        self.core
            .particles
            .initialize(&mut self.core.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        self.load_particle_textures();

        // The subsystems this scene uses, and the script modules they own
        // (#698), before the scripts that may call them load
        self.subsystems_build(entry);

        // Scripts: on_init with the state machine and the persistent store
        // in reach, then, at a transition, on_scene_enter
        load_scripts_from_world(
            &self.core.project,
            &self.core.scene_path,
            &mut self.core.script,
        );
        self.core
            .script
            .set_current_scene(&self.core.scene_path, &self.core.project);
        {
            let _state_scope = self.core.script.state_scope(
                &mut self.core.state_machine,
                &mut self.core.store,
                &self.core.physics,
            );
            self.core
                .script
                .initialize(&mut self.core.world)
                .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));
            if entry == SceneEntry::Transition {
                self.core.script.call_scene_enters(&mut self.core.world);
            }
        }
        Ok(())
    }
}
