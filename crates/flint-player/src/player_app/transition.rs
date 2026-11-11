//! Scene-transition lifecycle for `PlayerApp` — code-motion sibling of
//! `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the `TransitionPhase` state machine, chunk streaming, and the
//! transition executor (teardown ordering per ADR 0017: music session
//! stops before `audio.clear()`).

use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_scripts_from_world, load_sequences_from_world, load_sprite_animations_from_world,
    load_terrain_from_world_inner, register_node_animation_data, register_skeletal_data,
    resolve_scene_path,
};
use super::PlayerApp;
use flint_core::components as comp;
use flint_core::vfs;
use flint_ecs::FlintWorld;
use flint_render::model_loader;
use flint_runtime::RuntimeSystem;
use std::path::Path;

/// Scene transition lifecycle phase
#[derive(Debug, Clone)]
pub(super) enum TransitionPhase {
    /// Normal gameplay
    Idle,
    /// Playing exit transition — scripts draw fade-out visuals
    Exiting { target_scene: String, elapsed: f32 },
    /// Loading the new scene (synchronous, happens in one frame)
    Loading { target_scene: String },
    /// Playing enter transition — scripts draw fade-in visuals
    Entering { elapsed: f32 },
}

impl TransitionPhase {
    pub(super) fn is_idle(&self) -> bool {
        matches!(self, TransitionPhase::Idle)
    }
}

impl PlayerApp {
    /// Advance the transition phase based on elapsed time and script signals.
    pub(super) fn advance_transition(&mut self) {
        match &self.transition_phase {
            TransitionPhase::Idle => {}
            TransitionPhase::Exiting {
                target_scene,
                elapsed,
            } => {
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

    /// Load a chunk TOML file as additional entities with offset.
    pub(super) fn load_chunk(&mut self, path: &str, offset_x: f32, offset_y: f32, chunk_id: &str) {
        if self.loaded_chunks.contains_key(chunk_id) {
            tracing::warn!("Chunk '{}' already loaded, skipping", chunk_id);
            return;
        }

        // A path the script names: found by the project's one search
        // (`AssetKind::Files`), from the scene's directory first. A miss is
        // warned about with every path tried.
        let scene_dir = self.scene_dir();
        let chunk_path = self.project.file_or_beside("chunk", path, &scene_dir);

        // Parse chunk as a scene file
        let content = match vfs::read_to_string(&chunk_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to read chunk '{}': {}", chunk_path.display(), e);
                return;
            }
        };
        let scene_file: flint_scene::SceneFile = match toml::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to parse chunk '{}': {}", chunk_path.display(), e);
                return;
            }
        };

        // The registry the scene was loaded with, for archetype defaults:
        // kept on the app, never re-read for a chunk (#683)
        let registry = self.schemas.held_or_read(&self.schema_dirs.dirs);
        // What is wrong with the chunk's entities, said once (ADR 0115)
        flint_core::diagnostic::log_summary(
            &flint_core::project::shown(&chunk_path),
            &flint_scene::diagnose_scene(&scene_file, Some(&chunk_path), &registry),
        );

        let mut spawned_ids = Vec::new();

        // Spawn entities with prefixed names
        for (name, entity_def) in &scene_file.entities {
            let prefixed_name = format!("{}_{}", chunk_id, name);
            let id = match self.world.spawn(prefixed_name.clone()) {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!("Failed to spawn chunk entity '{}': {:?}", prefixed_name, e);
                    continue;
                }
            };
            spawned_ids.push(id);

            // Set archetype + defaults
            if let Some(archetype) = &entity_def.archetype {
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.archetype = Some(archetype.clone());
                    if let Some(arch_schema) = registry.get_archetype(archetype) {
                        for (comp_name, defaults) in &arch_schema.defaults {
                            if !comps.has(comp_name) {
                                comps.set(comp_name.clone(), defaults.clone());
                            }
                        }
                    }
                }
            }

            // Set component data
            for (comp_name, comp_data) in &entity_def.components {
                let _ = self.world.merge_component(id, comp_name, comp_data.clone());
            }

            // Apply position offset
            if let Some(transform) = self.world.get_transform(id) {
                let new_x = transform.position.x + offset_x;
                let new_y = transform.position.y + offset_y;
                let new_z = transform.position.z;
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.set_field(
                        comp::TRANSFORM,
                        "position",
                        toml::Value::Array(vec![
                            toml::Value::Float(new_x as f64),
                            toml::Value::Float(new_y as f64),
                            toml::Value::Float(new_z as f64),
                        ]),
                    );
                }
            } else {
                // Entity has no transform yet — create one with the offset
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.set_field(
                        comp::TRANSFORM,
                        "position",
                        toml::Value::Array(vec![
                            toml::Value::Float(offset_x as f64),
                            toml::Value::Float(offset_y as f64),
                            toml::Value::Float(0.0),
                        ]),
                    );
                }
            }
        }

        // Re-sync physics for new entities
        self.physics
            .sync_new_entities(&mut self.world, &spawned_ids);

        // Re-sync scripts for new entities: each found by the project's one
        // rule (`AssetKind::Scripts`), searched for from the chunk's folder
        // and, failing that, from the scene's.
        let chunk_dir = chunk_path.parent().unwrap_or_else(|| Path::new("."));
        let scene_dir = Path::new(&self.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let scene_dir = scene_dir.as_path();
        let sources: Vec<(flint_core::EntityId, String)> = spawned_ids
            .iter()
            .filter_map(|&id| {
                let source = self.world.get_components(id)?.get(comp::SCRIPT)?;
                let source = source.get("source")?.as_str()?;
                (!source.is_empty()).then(|| (id, source.to_string()))
            })
            .collect();
        for (id, source) in sources {
            // TEMPORARY, for one release (ADR 0114 decision 3, #765): a
            // chunk's script was `scripts/<source>` beside the chunk, else
            // beside the scene, and nowhere else.
            let old = [chunk_dir, scene_dir]
                .map(|dir| dir.join("scripts").join(&source))
                .into_iter()
                .find(|path| vfs::exists(path));
            let found = self
                .script
                .load_script_named(id, &source, chunk_dir)
                .or_else(|_| self.script.load_script_named(id, &source, scene_dir));
            match (&found, old) {
                (Err(not_found), None) => tracing::warn!("chunk '{chunk_id}': {not_found}"),
                (found, Some(old)) => {
                    let new = found.as_ref().ok().cloned();
                    if new.as_deref().map(flint_core::vfs::normalize)
                        != Some(flint_core::vfs::normalize(&old))
                    {
                        flint_core::project::OrderChange {
                            kind: flint_core::project::AssetKind::Scripts,
                            name: source.clone(),
                            old,
                            new,
                        }
                        .log();
                    }
                }
                (Ok(_), None) => {}
            }
        }

        // Initialize scripts for new entities
        self.script
            .initialize_entities(&mut self.world, &spawned_ids);

        println!(
            "[chunk] Loaded '{}' ({} entities) at offset ({}, {})",
            chunk_id,
            spawned_ids.len(),
            offset_x,
            offset_y
        );
        self.loaded_chunks.insert(chunk_id.to_string(), spawned_ids);
    }

    /// Unload a previously loaded chunk by despawning all its entities.
    pub(super) fn unload_chunk(&mut self, chunk_id: &str) {
        if let Some(entity_ids) = self.loaded_chunks.remove(chunk_id) {
            let count = entity_ids.len();
            for id in &entity_ids {
                self.physics.remove_entity(*id);
                self.script.remove_entity(*id);
                let _ = self.world.despawn(*id);
            }
            println!("[chunk] Unloaded '{}' ({} entities)", chunk_id, count);
        } else {
            tracing::warn!("Chunk '{}' not loaded, cannot unload", chunk_id);
        }
    }

    /// Unload the current scene and load a new one.
    pub(super) fn execute_scene_transition(&mut self, target_scene: &str) {
        // Teardown: scripts told, music session stopped BEFORE audio.clear()
        // (ADR 0017 producer→handles→device order), systems/world/terrain/
        // transient render state cleared.
        self.teardown_current_scene();

        // The outgoing scene's on_scene_exit has run: persist whatever it
        // (and the scene before it) stored, before the path changes.
        self.flush_persistent_store(true, "scene transition");

        // Load: resolve + schema discovery + scene parse; on failure the
        // transition aborts here (world already cleared, same as before).
        if !self.load_target_scene(target_scene) {
            return;
        }

        // Bring-up: camera, models, terrain, system re-init (audio before
        // the scene-declared music session, session before script init so
        // the conducted context exists from the scripts' first frame),
        // on_scene_enter, cursor.
        self.bring_up_scene();
    }

    fn teardown_current_scene(&mut self) {
        println!("[transition] Unloading current scene...");

        // Call on_scene_exit on all scripts
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script.call_scene_exits(&mut self.world);
        }

        // Music session first: capture guard drops and stems stop BEFORE the
        // audio system clears (ADR 0017 producer→handles→device order).
        self.stop_music_session();

        // Clear all systems
        self.script.clear();
        self.audio.clear();
        self.physics.clear();
        self.animation.clear();
        self.particles.clear();
        self.procgen_resolver.clear_queue();
        self.procgen_resolver.clear_living_props();

        // Clear world
        self.world = FlintWorld::new();

        // Clear terrain
        self.terrain = None;
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.clear_terrain();
            // Entity ids are never reused, so the old scene's bones are garbage.
            renderer.clear_entity_bone_buffers();
            renderer.unload_grass();
        }

        // Clear transient rendering state
        self.skeletal_entity_assets.clear();
        self.ui_textures.clear();
        self.draw_commands.clear();
        self.loaded_chunks.clear();
    }

    /// The registry for `scene`, after asking the one resolver (ADR 0114)
    /// what the directories are now: flags stay, a manifest's list is read
    /// again (the scene may belong to another project), a discovered or
    /// host list gains what lies beside the scene. A list that can't be
    /// named (a manifest naming a missing directory) is logged, and the
    /// directories already held are kept.
    ///
    /// The registry is the one the app holds (#683): the same directories
    /// are not read again, and directories that fail to read leave it as it
    /// was, so the scene never loads with an empty registry.
    fn schemas_for_scene(&mut self, scene: &str) -> std::sync::Arc<flint_schema::SchemaRegistry> {
        match self.schema_dirs.for_scene(Path::new(scene)) {
            Ok(dirs) => self.schema_dirs = dirs,
            Err(e) => tracing::error!("{e}; keeping the schema directories already loaded"),
        }
        self.schemas.for_dirs(&self.schema_dirs.dirs)
    }

    fn load_target_scene(&mut self, target_scene: &str) -> bool {
        println!("[transition] Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let new_scene_path = resolve_scene_path(&self.project, &self.scene_path, target_scene);

        // The new scene's schema directories and its registry
        let registry = self.schemas_for_scene(&new_scene_path);

        // Parse and load scene
        // Living props' tables are checked and their preset filled in, as at
        // the first load (#686)
        let loaded = flint_scene::load_scene(&new_scene_path, &registry).and_then(
            |(world, mut scene_file)| {
                let path = std::path::Path::new(&new_scene_path);
                flint_procgen::settle_scene(&mut scene_file, path)?;
                Ok((world, scene_file))
            },
        );
        match loaded {
            Ok((world, scene_file)) => {
                self.world = world;
                self.scene_path = new_scene_path.clone();
                // The new scene may belong to another project (ADR 0114)
                self.refresh_project();
                // A slow-motion moment never outlives the scene that asked for it
                self.clock.time_scale = 1.0;
                self.skybox_path = scene_file
                    .environment
                    .as_ref()
                    .and_then(|env| env.skybox.clone());
                self.scene_ambient = scene_file.environment.as_ref().and_then(|env| {
                    (env.ambient_sky.is_some() || env.ambient_ground.is_some()).then(|| {
                        (
                            env.ambient_sky
                                .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_SKY),
                            env.ambient_ground
                                .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_GROUND),
                        )
                    })
                });
                let env = scene_file.environment.as_ref();
                self.scene_shadows = env.is_none_or(|e| e.casts_shadows());
                self.scene_default_lights = env.is_none_or(|e| e.uses_default_lights());
                self.scene_diffuse_wrap = scene_file
                    .environment
                    .as_ref()
                    .and_then(|env| env.diffuse_wrap);
                self.scene_oren_nayar = scene_file
                    .environment
                    .as_ref()
                    .and_then(|env| env.oren_nayar);
                self.scene_sheen = scene_file.environment.as_ref().and_then(|env| {
                    env.sheen_strength
                        .map(|s| (env.sheen_color.unwrap_or([1.0; 3]), s))
                });
                self.scene_camera = scene_file.camera.clone();
                super::scene_loading::load_user_styles(&new_scene_path);
                self.scene_post_process = super::scene_loading::scene_post_process(
                    self.look.as_deref(),
                    &new_scene_path,
                    scene_file.post_process.clone(),
                );
                let living_tables = flint_procgen::LivingTables::or_warn(&scene_file);
                self.scene_living = living_tables.living;
                self.scene_wooze = living_tables.wooze;
                self.scene_tables = scene_file.scene_tables();
                self.scene_input_config = scene_file.scene.input_config.clone();
                self.scene_preload_audio = scene_file.scene.preload_audio;
            }
            Err(e) => {
                tracing::error!("Failed to load scene '{}': {:?}", new_scene_path, e);
                return false;
            }
        }

        // Rebuild component index after loading new scene
        self.world.rebuild_component_index();

        true
    }

    fn bring_up_scene(&mut self) {
        // Apply camera config before borrowing renderer
        self.apply_camera_def();
        // The new scene's [post_process] with --style over it (#328)
        let scene_post_config = self.scene_post_config();

        // Reload models
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            let scene_dir = Path::new(&self.scene_path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            renderer.set_asset_scope(flint_core::project::AssetScope::new(
                self.project.clone(),
                scene_dir,
            ));

            let config = build_model_load_config(
                &self.project,
                &self.scene_path,
                &self.world,
                self.catalog.as_ref(),
                self.content_store.as_ref(),
            );

            // Procgen before the model loader: catalog > procgen > file (#32).
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
            super::procgen_assets::resolve_scene(
                &self.scene_path,
                &mut self.world,
                &mut self.procgen_resolver,
                self.procgen_mode,
                super::procgen_assets::GpuSink {
                    renderer,
                    device: &context.device,
                    queue: &context.queue,
                    config: &config,
                },
            );

            let load_result = model_loader::load_models_from_world(
                &mut self.world,
                renderer,
                &context.device,
                &context.queue,
                &config,
            );
            register_skeletal_data(&load_result, &mut self.animation);
            register_node_animation_data(&load_result, &mut self.animation);
            self.skeletal_entity_assets = load_result.skinned_entities;
            renderer.update_from_world(&self.world, &context.device);

            // Reload splines
            crate::spline_gen::load_splines(
                &self.scene_path,
                &mut self.world,
                renderer,
                Some(&mut self.physics),
                &context.device,
                &context.queue,
            );
            renderer.update_from_world(&self.world, &context.device);
            // The new scene's first frame has no past: styles that keep
            // history (ASCII's temporal stability, #277) start over
            renderer.reset_style_history();
            // ...and the new scene's first camera is no cut from the old one's
            self.camera_cut.forget();

            // Scene-authored hemisphere ambient + wrap; reset first so the
            // previous scene's values never leak into one that doesn't
            // author them (reset clears both ambient and wrap)
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

            // Reload skybox
            if let Some(skybox_rel) = &self.skybox_path {
                let scene_dir = Path::new(&self.scene_path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let skybox_path = self.project.file_or_beside("skybox", skybox_rel, scene_dir);
                if vfs::exists(&skybox_path) {
                    renderer.load_skybox(&context.device, &context.queue, &skybox_path);
                }
            }

            // Apply post-process config
            if let Some(config) = scene_post_config {
                renderer.set_post_process_config(config);
                renderer.ensure_style_resources(&context.device);
                renderer.ensure_fxaa_resources(&context.device);
            }
        }

        // Reload terrain
        // The scene panels go with the scene. Rendering & Effects (F4) and
        // the game's own panels belong to the player and are kept (open or
        // closed as they were); before #268 they were dropped here and
        // never made again, so F4 went dead after the first transition.
        #[cfg(feature = "debug-hud")]
        self.drop_scene_debug_panels();
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            let mut grass_info = None;
            load_terrain_from_world_inner(
                &self.world,
                &self.scene_path,
                &context.device,
                &context.queue,
                renderer,
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
        }
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
            self.create_styles_debug_panel();
            self.create_render_debug_panel();
            self.create_extension_debug_panels();
        }
        self.apply_camera_tuning();

        // Update terrain height callback for scripts
        self.update_terrain_height_fn();

        // Project fonts may differ when the new scene lives in another project
        self.refresh_project_fonts();

        // Re-initialize systems
        self.physics
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Physics init failed: {:?}", e));

        load_audio_from_world(
            &self.world,
            &mut self.audio,
            &self.project,
            &self.scene_path,
            self.scene_preload_audio,
            // In the browser the audio may already be here at a scene change, and
            // decoding it all in one frame is the stall #124 removed:
            // queue it. The desktop decodes up front, as a load can afford.
            cfg!(target_arch = "wasm32"),
        );
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        // Scene-declared music session on the freshly initialized audio
        // (before scripts, so a conducted context exists from their first
        // frame — F4). Music→music transitions: the fresh gilrs above is
        // simply dropped again by the new session's handoff.
        self.start_music_session();

        load_animations_from_world(&self.project, &self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.project, &self.scene_path, &mut self.animation);
        load_sequences_from_world(&self.project, &self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        flint_particles::load_particle_effects_from_world(
            &self.project,
            &self.scene_path,
            &mut self.particles,
        );
        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        self.load_particle_textures();

        load_scripts_from_world(&self.project, &self.scene_path, &mut self.script);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));

        // Call on_scene_enter on new scripts
        self.script
            .set_current_scene(&self.scene_path, &self.project);
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script.call_scene_enters(&mut self.world);
        }

        // Recapture cursor if player exists
        if self.physics.has_player_entity() && !self.cursor_captured {
            self.capture_cursor();
        }

        println!("[transition] Scene loaded: {}", self.scene_path);
    }
}
