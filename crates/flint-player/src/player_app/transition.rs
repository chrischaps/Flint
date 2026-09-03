//! Scene-transition lifecycle for `PlayerApp` — code-motion sibling of
//! `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the `TransitionPhase` state machine, chunk streaming, and the
//! transition executor (teardown ordering per ADR 0017: music session
//! stops before `audio.clear()`).

use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_scripts_from_world, load_sequences_from_world, load_sprite_animations_from_world,
    load_terrain_from_world_inner, register_node_animation_data, register_skeletal_data,
    resolve_procgen_assets, resolve_scene_path,
};
use super::PlayerApp;
use flint_core::components as comp;
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

        // Resolve path relative to scene directory
        let scene_dir = Path::new(&self.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let chunk_path = scene_dir.join(path);

        // Parse chunk as a scene file
        let content = match std::fs::read_to_string(&chunk_path) {
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

        // Load schema registry for archetype defaults
        let registry = if self.schema_paths.is_empty() {
            flint_schema::SchemaRegistry::load_from_directory("schemas")
                .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
        } else {
            let existing: Vec<&str> = self
                .schema_paths
                .iter()
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

        // Re-sync scripts for new entities
        let chunk_scripts_dir = chunk_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("scripts");
        for &id in &spawned_ids {
            if let Some(comps) = self.world.get_components(id) {
                if let Some(script_comp) = comps.get(comp::SCRIPT) {
                    if let Some(source) = script_comp.get("source").and_then(|v| v.as_str()) {
                        // Try chunk's own scripts dir first, then scene's scripts dir
                        let scene_scripts_dir = Path::new(&self.scene_path)
                            .parent()
                            .unwrap_or_else(|| Path::new("."))
                            .join("scripts");
                        let script_path = if chunk_scripts_dir.join(source).exists() {
                            chunk_scripts_dir.join(source)
                        } else {
                            scene_scripts_dir.join(source)
                        };
                        if script_path.exists() {
                            self.script.load_script_for_entity(id, &script_path);
                        }
                    }
                }
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

        // Clear world
        self.world = FlintWorld::new();

        // Clear terrain
        self.terrain = None;
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.clear_terrain();
        }

        // Clear transient rendering state
        self.skeletal_entity_assets.clear();
        self.ui_textures.clear();
        self.draw_commands.clear();
        self.loaded_chunks.clear();
    }

    fn load_target_scene(&mut self, target_scene: &str) -> bool {
        println!("[transition] Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let new_scene_path = resolve_scene_path(&self.scene_path, target_scene);

        // Auto-discover schema dirs from the new scene path and merge
        for dir in flint_schema::discover_schema_dirs(&new_scene_path) {
            let s = dir.to_string_lossy().into_owned();
            if !self.schema_paths.contains(&s) {
                self.schema_paths.push(s);
            }
        }

        // Load schema registry
        let registry = if self.schema_paths.is_empty() {
            // Try default schemas/ dir
            flint_schema::SchemaRegistry::load_from_directory("schemas")
                .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
        } else {
            let existing: Vec<&str> = self
                .schema_paths
                .iter()
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
                self.scene_post_process = scene_file.post_process.clone();
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

        // Reload models
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.scene_dir = Path::new(&self.scene_path)
                .parent()
                .map(|p| p.to_path_buf());

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
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                );
            }

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
            );
            renderer.update_from_world(&self.world, &context.device);

            // Scene-authored hemisphere ambient + wrap; reset first so the
            // previous scene's values never leak into one that doesn't
            // author them (reset clears both ambient and wrap)
            renderer.reset_ambient();
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
                renderer.set_post_process_config(
                    super::scene_loading::post_process_config_from_def(pp_def),
                );
                renderer.ensure_kuwahara_resources(&context.device, &context.queue);
                renderer.ensure_fxaa_resources(&context.device);
            }
        }

        // Reload terrain
        #[cfg(feature = "debug-hud")]
        self.debug_panels.clear();
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
                );
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
        }
        self.apply_camera_tuning();

        // Update terrain height callback for scripts
        self.update_terrain_height_fn();

        // Re-initialize systems
        self.physics
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Physics init failed: {:?}", e));

        load_audio_from_world(
            &self.world,
            &mut self.audio,
            &self.scene_path,
            self.scene_preload_audio,
        );
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        // Scene-declared music session on the freshly initialized audio
        // (before scripts, so a conducted context exists from their first
        // frame — F4). Music→music transitions: the fresh gilrs above is
        // simply dropped again by the new session's handoff.
        self.start_music_session();

        load_animations_from_world(&self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.scene_path, &mut self.animation);
        load_sequences_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        self.load_particle_textures();

        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));

        // Call on_scene_enter on new scripts
        self.script.set_current_scene(&self.scene_path);
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
