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
    register_node_animation_data, register_skeletal_data, register_terrain_collider,
    LoadedGrassInfo,
};
use super::PlayerApp;
use anyhow::{Context, Result};
use flint_core::frame_notes::{self, NoteKind};
use flint_core::load_report::{self, Stage};
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
        flint_core::scope!("enter_scene");
        // Which project the scene is in, before anything is searched for
        self.refresh_project();

        // The scene's `[scene] input_config`, else the project's, under the
        // player's own rebinds and the CLI's
        self.configure_input_bindings()
            .unwrap_or_else(|e| tracing::warn!("Input config load error: {e:#}"));

        // A fresh camera, then the scene's `[camera]` over it
        self.reset_camera();

        // Project fonts may differ when the scene lives in another project
        load_report::timed(Stage::Hud, String::new, || {
            self.refresh_project_fonts();
            if entry == SceneEntry::Start {
                self.install_text_measurer();
            }
        });

        // Which model each mesh collider takes its triangles from, before
        // the renderer expands a model of several nodes and its parent
        // loses its `model` (#729)
        let collider_meshes = super::collider_meshes::requests(&self.core.world);

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
            // The player's own bus volumes (#1217), before the scene's
            // sounds load and its scripts start, so the first note and
            // on_init both have them. Placed by the same id as the saves.
            self.load_user_audio();
        }

        // The project's collision layers and the mesh colliders' triangles,
        // before physics makes the scene's bodies
        frame_notes::timed(NoteKind::PhysicsBuild, String::new, || {
            self.prepare_physics(&collider_meshes)
        });

        self.start_scene_systems(entry)?;
        // The subsystems this scene uses, each told it is up
        self.subsystems_enter_scene();
        // A `[debug_keys]` name nothing has, or two actions on one key (#717)
        self.say_debug_key_config();
        // One line for what loaded and what went wrong (the line above's
        // warnings included), and one for the controls this scene binds (#716)
        self.say_loaded();
        self.say_controls();
        // The load report's bring-up ends here; the first frame is next (#1092)
        self.load_brought_up();

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
        // What spawns from here on is late, and its models load on their
        // own (#1107); what the staging spawns is the scene's
        self.late_models.enter_scene(&self.core.world);
        let context = self.render_context.take();
        let mut renderer = self.scene_renderer.take();
        let gpu = context.as_ref().map(|c| (&c.device, &c.queue));
        let gpu = gpu.or(self.windowless_gpu().map(|g| (&g.device, &g.queue)));
        let grass = match (renderer.as_mut(), gpu) {
            (Some(renderer), Some((device, queue))) => self.stage_scene(renderer, device, queue),
            _ => None,
        };
        self.render_context = context;
        self.scene_renderer = renderer;
        self.late_models.skip_to(&self.core.world);
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
        // The scene, bound (#739): its project and folder, which the
        // renderer's styles find palettes, glyph sets and papers from (ADR
        // 0114); no style history, bones or levers of the last scene; and
        // its terrain and grass; its archetypes' placeholder boxes, from the
        // registry its world was made with (#751)
        let schemas = self.schemas.held_or_read(&self.schema_dirs.dirs);
        let binding = self
            .scene_binding
            .take()
            .unwrap_or_else(|| flint_render::SceneBinding::new(self.kept_asset_scope()))
            .with_archetypes(&schemas);
        let bound = renderer.bind_scene(device, queue, &self.core.world, &binding);
        // The core's registry is the scene's (#742), and a game style an
        // extension added since the binding was made is in it
        renderer.set_styles(self.core.styles.clone());

        let config = build_model_load_config(
            &self.core.project,
            &self.core.scene_path,
            &self.core.world,
            self.assets.of(&self.core.project),
        );

        // What the scene says about living props, for the props procgen
        // adds (their clocks and field are the living subsystem's)
        #[cfg(feature = "living")]
        self.core
            .living_props
            .set_defaults(super::living_props::defaults_of(self.scene.living.as_ref()));

        // Procgen before the model loader: catalog > procgen > file (#32).
        // A build without it (#754) says what it won't make.
        #[cfg(not(feature = "procgen"))]
        if let Some(line) = super::missing_features::procgen_line(
            &self.core.world,
            &self.core.project,
            &self.core.scene_path,
        ) {
            tracing::warn!(target: "load", "{line}");
        }
        #[cfg(feature = "procgen")]
        super::procgen_assets::resolve_scene(
            &mut self.core,
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
        self.late_models.remember_expanded(&load_result);
        self.skeletal_entity_assets = load_result.skinned_entities;

        // Meshes swept along the scene's splines (#750), each with its
        // collider unless it says `collider = false`
        let splines = flint_render::load_scene_splines(
            &self.core.scene_path,
            &mut self.core.world,
            renderer,
            device,
            queue,
        );
        for c in splines.colliders {
            self.core.physics.register_static_trimesh(
                c.entity,
                c.vertices,
                c.triangles,
                c.friction,
                c.restitution,
            );
        }
        // The scene's draws, read once its models and splines are in (#739:
        // the frame's own inputs replace these at its first sync)
        renderer.prepare(
            device,
            queue,
            &self.core.world,
            &flint_render::FrameInputs {
                time: self.core.clock.total_time,
                look_time: self.launch.style_time.map(f64::from),
                ..Default::default()
            },
        );

        // The scene's first frame has no past (`bind_scene` started styles'
        // history over, #277), and its first camera is no cut from the last
        // scene's
        self.style.camera_cut.forget();

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
        // What a later frame would build on the frame it first shows:
        // the stage draws, `preload_looks`, `preload_styles` (#1159)
        self.preload_scene_styles(renderer, device);
        // Comic panels (#880, ADR 0120): the scene's [panels], or one view,
        // replacing the last scene's layout and its panels' views
        renderer.set_panels(
            flint_render::panels::PanelLayout::from_scene_tables(
                &self.scene.tables,
                &self.core.scene_path,
            ),
            self.scene.post_process.as_ref(),
        );

        // The terrain `bind_scene` loaded: its collider, and its heights for
        // scripts
        if let Some(loaded) = bound.terrain {
            register_terrain_collider(&mut self.core.physics, &loaded);
            self.terrain = Some((loaded.terrain, loaded.config));
        }
        bound.grass.map(|grass| LoadedGrassInfo {
            config: grass.config,
            terrain_entity_name: grass.entity_name,
            component: grass.component,
        })
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
        // A game's controllers, from their schemas (#744)
        self.create_component_panels();
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
        // Each system's start is a frame note (#1092): a transition's
        // hitch names them, and a load report times them
        let physics = frame_notes::timed(NoteKind::PhysicsBuild, String::new, || {
            self.core.physics.initialize(&mut self.core.world)
        })
        .context("Failed to initialize physics");
        match (physics, entry) {
            (Err(e), SceneEntry::Start) => return Err(e),
            (Err(e), SceneEntry::Transition) => tracing::warn!("{e:#}"),
            (Ok(()), _) => {}
        }

        let audio_note = frame_notes::begin(NoteKind::AudioLoad, String::new);
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
        drop(audio_note);

        let animation_note = frame_notes::begin(NoteKind::AnimationLoad, String::new);
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
        drop(animation_note);

        // `particles/*.particles.toml` registered first, so `particle_effect`
        // components resolve on the first sync
        self.refresh_project();
        let particle_note = frame_notes::begin(NoteKind::ParticleLoad, String::new);
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
        drop(particle_note);

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
        // A playtest's own checks (#1192), on entities of their own
        self.attach_test_scripts();
        self.core
            .script
            .set_current_scene(&self.core.scene_path, &self.core.project);
        // The looks the scene can see and the one it draws with, for
        // on_init's list_looks() and get_look() (#497)
        self.publish_looks();
        {
            let _init = frame_notes::begin(NoteKind::ScriptInit, String::new);
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
