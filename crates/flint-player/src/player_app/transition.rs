//! Scene-transition lifecycle for `PlayerApp` — code-motion sibling of
//! `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the `TransitionPhase` state machine, chunk streaming, and the
//! transition executor (teardown ordering per ADR 0017: music session
//! stops before `audio.clear()`).

use super::scene_loading::resolve_scene_path;
use super::PlayerApp;
use flint_core::components as comp;
use flint_core::vfs;
use flint_ecs::FlintWorld;
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

/// How long, in game seconds, an Exiting or Entering phase waits for a
/// script's `complete_transition()` before the player ends it itself with a
/// warning (#362). The fades scripts draw take a second or two
/// (`showcase_hud.rhai`: 1.5 s out, 2 s in); a phase this long is a script
/// that meant to finish it and didn't, not a fade.
pub(super) const TRANSITION_PHASE_TIMEOUT: f32 = 10.0;

impl PlayerApp {
    /// Advance the transition phase based on elapsed time and script signals.
    ///
    /// A script ends the Exiting and Entering phases with
    /// `complete_transition()` (`fire_event`'s `TRANSITION_COMPLETE`), once
    /// its fade is drawn. A phase no loaded script can end (none names
    /// `complete_transition` or imports a module) moves on here by itself,
    /// so a bare `load_scene` reaches its scene on the next frame (#362).
    /// One a script can end but doesn't is ended after
    /// [`TRANSITION_PHASE_TIMEOUT`], with a warning.
    pub(super) fn advance_transition(&mut self) {
        match &self.transition_phase {
            TransitionPhase::Idle => {}
            TransitionPhase::Exiting {
                target_scene,
                elapsed,
            } => {
                let target = target_scene.clone();
                let elapsed = elapsed + self.clock.delta_time as f32;
                let over = if !self.script.can_complete_transition() {
                    println!("[transition] No script completes transitions: loading now");
                    true
                } else if elapsed >= TRANSITION_PHASE_TIMEOUT {
                    tracing::warn!(
                        "[transition] still exiting after {TRANSITION_PHASE_TIMEOUT} s with no \
                         complete_transition() from a script: loading {target} now"
                    );
                    true
                } else {
                    false
                };
                if over {
                    // Loaded this frame, as a script's complete_transition
                    // last frame would have it
                    self.transition_phase = TransitionPhase::Loading {
                        target_scene: target,
                    };
                    self.advance_transition();
                } else {
                    self.transition_phase = TransitionPhase::Exiting {
                        target_scene: target,
                        elapsed,
                    };
                }
            }
            TransitionPhase::Loading { target_scene } => {
                let target = target_scene.clone();
                // Either way the scripts fade back in: over the new scene,
                // or over this one when the next didn't load (#723)
                self.execute_scene_transition(&target);
                // ...if one of them can say when it's done (#362)
                self.transition_phase = if self.script.can_complete_transition() {
                    TransitionPhase::Entering { elapsed: 0.0 }
                } else {
                    println!("[transition] Transition complete");
                    TransitionPhase::Idle
                };
            }
            TransitionPhase::Entering { elapsed } => {
                let elapsed = elapsed + self.clock.delta_time as f32;
                self.transition_phase = if !self.script.can_complete_transition() {
                    // A hot reload took the call out
                    println!("[transition] Transition complete");
                    TransitionPhase::Idle
                } else if elapsed >= TRANSITION_PHASE_TIMEOUT {
                    tracing::warn!(
                        "[transition] still entering after {TRANSITION_PHASE_TIMEOUT} s with no \
                         complete_transition() from a script: ending the transition"
                    );
                    TransitionPhase::Idle
                } else {
                    TransitionPhase::Entering { elapsed }
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

    /// Leave the current scene for `target_scene`. The target is read first:
    /// a scene that can't be loaded leaves this one playing, logs the path
    /// it was read from and why, and makes the run end non-zero
    /// (`run_failure`, #723). Returns whether the new scene was entered.
    pub(super) fn execute_scene_transition(&mut self, target_scene: &str) -> bool {
        let next = match self.read_scene(target_scene) {
            Ok(next) => next,
            Err(e) => {
                tracing::error!("[transition] {e}; staying in {}", self.scene_path);
                if self.outcome.scene_failure.is_none() {
                    self.outcome.scene_failure = Some(e);
                }
                return false;
            }
        };

        // Teardown: scripts told, music session stopped BEFORE audio.clear()
        // (ADR 0017 producer→handles→device order), systems/world/terrain/
        // transient render state cleared.
        self.teardown_current_scene();

        // The outgoing scene's on_scene_exit has run: persist whatever it
        // (and the scene before it) stored, before the path changes.
        self.flush_persistent_store(true, "scene transition");

        self.install_scene(next);

        // The one bring-up startup uses too (`scene_entry.rs`)
        if let Err(e) = self.enter_scene(super::scene_entry::SceneEntry::Transition) {
            tracing::warn!("[transition] {e:#}");
        }
        println!("[transition] Scene loaded: {}", self.scene_path);
        true
    }

    fn teardown_current_scene(&mut self) {
        println!("[transition] Unloading current scene...");

        // Call on_scene_exit on all scripts
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.saves.store,
                &self.physics,
            );
            self.script.call_scene_exits(&mut self.world);
        }
        // Each subsystem lets go of the scene, before anything is cleared
        self.subsystems_leave_scene();

        // Music session first: capture guard drops and stems stop BEFORE the
        // audio system clears (ADR 0017 producer→handles→device order).
        self.stop_music_session();
        // The session's post restore belongs to this scene: the next one's
        // config replaces the renderer's whole (`stage_scene`), and a restore
        // landing after it would stamp this scene's values over it
        self.music.pp_restore = None;
        // ...as does a render mode a script held on: the next frame would
        // otherwise switch the new scene's authored mode off
        self.post.mode_was_active = false;

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
        self.hud.textures.clear();
        self.hud.draw_commands.clear();
        self.hud.world_texts.clear();
        self.loaded_chunks.clear();
        // Style tweens name the old scene's entities (#370)
        self.style.tweens.clear();
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

    /// Resolve and load `target_scene` without touching the scene playing.
    /// The error names the target as written, the path it was read from
    /// and why it failed.
    fn read_scene(&mut self, target_scene: &str) -> Result<NextScene, String> {
        println!("[transition] Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let path = resolve_scene_path(&self.project, &self.scene_path, target_scene);

        // The new scene's schema directories and its registry; kept as they
        // were when the scene doesn't load
        let schema_dirs = self.schema_dirs.clone();
        let registry = self.schemas_for_scene(&path);

        // Living props' tables are checked and their preset filled in, as at
        // the first load (#686): a scene that won't settle doesn't load
        let loaded = flint_scene::load_scene(&path, &registry).and_then(|(world, mut file)| {
            flint_procgen::settle_scene(&mut file, Path::new(&path))?;
            Ok((world, file))
        });
        match loaded {
            Ok((world, file)) => Ok(NextScene { path, world, file }),
            Err(e) => {
                self.schema_dirs = schema_dirs;
                Err(format!(
                    "scene '{target_scene}' failed to load from {}: {e}",
                    flint_core::project::shown(Path::new(&path))
                ))
            }
        }
    }

    /// Make `next` the current scene: its world and path, its project, and
    /// every setting its file carries (`apply_scene_settings`, the same call
    /// the hosts make for the first scene).
    fn install_scene(&mut self, next: NextScene) {
        self.world = next.world;
        self.scene_path = next.path;
        // The new scene may belong to another project (ADR 0114)
        self.refresh_project();
        // A slow-motion moment never outlives the scene that asked for it
        self.clock.time_scale = 1.0;
        self.apply_scene_settings(&next.file);
    }
}

/// A scene read for a transition, not yet entered.
struct NextScene {
    /// Resolved: where it was read from
    path: String,
    world: FlintWorld,
    file: flint_scene::SceneFile,
}
