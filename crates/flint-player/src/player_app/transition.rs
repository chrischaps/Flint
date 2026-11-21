//! Scene-transition lifecycle for `PlayerApp` — code-motion sibling of
//! `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the `TransitionPhase` state machine, chunk streaming, and the
//! transition executor (teardown ordering per ADR 0017: a subsystem
//! lets go before `audio.clear()`).

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
    /// Loading the new scene (synchronous, happens in one frame).
    /// `covered`: a script ended the Exiting phase with
    /// `complete_transition()`, so the load's long frame is drawn under its
    /// fade and its hitch is counted as `loading` (#1125). A transition no
    /// script covers (it goes straight through) or one the timeout ended
    /// is not: the player may see the stall.
    Loading { target_scene: String, covered: bool },
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
                let elapsed = elapsed + self.core.clock.delta_time as f32;
                let over = if !self.core.script.can_complete_transition() {
                    tracing::info!(target: "transition", "No script completes transitions: loading now");
                    true
                } else if elapsed >= TRANSITION_PHASE_TIMEOUT {
                    tracing::warn!(target: "transition",
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
                    // No cover: nothing drew one, or nothing said it was
                    // drawn
                    self.transition_phase = TransitionPhase::Loading {
                        target_scene: target,
                        covered: false,
                    };
                    self.advance_transition();
                } else {
                    self.transition_phase = TransitionPhase::Exiting {
                        target_scene: target,
                        elapsed,
                    };
                }
            }
            TransitionPhase::Loading {
                target_scene,
                covered,
            } => {
                let target = target_scene.clone();
                if *covered {
                    self.stats.perf.loading_behind_cover();
                }
                // Either way the scripts fade back in: over the new scene,
                // or over this one when the next didn't load (#723)
                self.execute_scene_transition(&target);
                // ...if one of them can say when it's done (#362)
                self.transition_phase = if self.core.script.can_complete_transition() {
                    TransitionPhase::Entering { elapsed: 0.0 }
                } else {
                    tracing::info!(target: "transition", "Transition complete");
                    TransitionPhase::Idle
                };
            }
            TransitionPhase::Entering { elapsed } => {
                let elapsed = elapsed + self.core.clock.delta_time as f32;
                self.transition_phase = if !self.core.script.can_complete_transition() {
                    // A hot reload took the call out
                    tracing::info!(target: "transition", "Transition complete");
                    TransitionPhase::Idle
                } else if elapsed >= TRANSITION_PHASE_TIMEOUT {
                    tracing::warn!(target: "transition",
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
            tracing::warn!(target: "transition", "Chunk '{}' already loaded, skipping", chunk_id);
            return;
        }
        flint_core::scope!("load_chunk", chunk_id);
        // A chunk streamed in has a table of its own, unless it arrives
        // inside another load (a scene's on_init), which it is part of
        let own_report = self.launch.load_report && !flint_core::load_report::recording();
        if own_report {
            self.begin_load_report(flint_core::load_report::LoadKind::Chunk, chunk_id);
        }
        let _note =
            flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::ChunkLoad, || {
                format!("{chunk_id} ({path})")
            });

        // A path the script names: found by the project's one search
        // (`AssetKind::Files`), from the scene's directory first. A miss is
        // warned about with every path tried.
        let scene_dir = self.scene_dir();
        let chunk_path = self.core.project.file_or_beside("chunk", path, &scene_dir);

        // Parse chunk as a scene file
        let content = match vfs::read_to_string(&chunk_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(target: "transition", "Failed to read chunk '{}': {}", chunk_path.display(), e);
                return;
            }
        };
        let scene_file: flint_scene::SceneFile = match toml::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(target: "transition", "Failed to parse chunk '{}': {}", chunk_path.display(), e);
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
            let id = match self.core.world.spawn(prefixed_name.clone()) {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!(target: "transition", "Failed to spawn chunk entity '{}': {:?}", prefixed_name, e);
                    continue;
                }
            };
            spawned_ids.push(id);

            // Set archetype + defaults
            if let Some(archetype) = &entity_def.archetype {
                if let Some(mut comps) = self.core.world.get_components_mut(id) {
                    comps.set_archetype(Some(archetype.clone()));
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
                let _ = self
                    .core
                    .world
                    .merge_component(id, comp_name, comp_data.clone());
            }

            // Apply position offset
            if let Some(transform) = self.core.world.get_transform(id) {
                let new_x = transform.position.x + offset_x;
                let new_y = transform.position.y + offset_y;
                let new_z = transform.position.z;
                if let Some(mut comps) = self.core.world.get_components_mut(id) {
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
                if let Some(mut comps) = self.core.world.get_components_mut(id) {
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

        // Re-sync physics for new entities, the chunk's mesh colliders
        // given their models' triangles first (#729)
        let collider_meshes: Vec<_> = super::collider_meshes::requests(&self.core.world)
            .into_iter()
            .filter(|(id, _)| spawned_ids.contains(id))
            .collect();
        // The chunk's models (#1107), after the colliders have asked for
        // their triangles (a model of several nodes leaves its parent with
        // no `model`) and inside the chunk's note and load report
        self.load_late_models();
        self.prepare_physics(&collider_meshes);
        self.core
            .physics
            .sync_new_entities(&mut self.core.world, &spawned_ids);

        // Re-sync scripts for new entities: each found by the project's one
        // rule (`AssetKind::Scripts`), searched for from the chunk's folder
        // and, failing that, from the scene's.
        let chunk_dir = chunk_path.parent().unwrap_or_else(|| Path::new("."));
        let scene_dir = Path::new(&self.core.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let scene_dir = scene_dir.as_path();
        let sources: Vec<(flint_core::EntityId, String)> = spawned_ids
            .iter()
            .filter_map(|&id| {
                let source = self.core.world.view(id, comp::SCRIPT).opt_str("source")?;
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
                .core
                .script
                .load_script_named(id, &source, chunk_dir)
                .or_else(|_| self.core.script.load_script_named(id, &source, scene_dir));
            match (&found, old) {
                (Err(not_found), None) => {
                    tracing::warn!(target: "transition", "chunk '{chunk_id}': {not_found}")
                }
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
        self.core
            .script
            .initialize_entities(&mut self.core.world, &spawned_ids);

        tracing::info!(target: "transition", chunk = chunk_id, entities = spawned_ids.len(),
            "Loaded chunk '{}' ({} entities) at offset ({}, {})",
            chunk_id,
            spawned_ids.len(),
            offset_x,
            offset_y
        );
        self.loaded_chunks.insert(chunk_id.to_string(), spawned_ids);
        if own_report {
            flint_core::load_report::set_name(chunk_id);
            flint_core::load_report::bring_up_done();
        }
    }

    /// Unload a previously loaded chunk by despawning all its entities.
    pub(super) fn unload_chunk(&mut self, chunk_id: &str) {
        if let Some(entity_ids) = self.loaded_chunks.remove(chunk_id) {
            let count = entity_ids.len();
            for id in &entity_ids {
                self.core.physics.remove_entity(*id);
                self.core.script.remove_entity(*id);
                let _ = self.core.world.despawn(*id);
            }
            tracing::info!(target: "transition", chunk = chunk_id, entities = count, "Unloaded chunk '{}' ({} entities)", chunk_id, count);
        } else {
            tracing::warn!(target: "transition", "Chunk '{}' not loaded, cannot unload", chunk_id);
        }
    }

    /// Leave the current scene for `target_scene`. The target is read first:
    /// a scene that can't be loaded leaves this one playing, logs the path
    /// it was read from and why, and makes the run end non-zero
    /// (`run_failure`, #723). Returns whether the new scene was entered.
    pub(super) fn execute_scene_transition(&mut self, target_scene: &str) -> bool {
        flint_core::scope!("scene_transition", target_scene);
        // Its load report, to the new scene's first frame (#1092)
        self.begin_load_report(flint_core::load_report::LoadKind::Transition, target_scene);
        // One frame does all of it: a frame note (#1077), with the models,
        // textures, scripts and pipelines it loaded noted inside it
        let _note = flint_core::frame_notes::begin(
            flint_core::frame_notes::NoteKind::SceneTransition,
            || target_scene.to_string(),
        );
        // The new scene's load summary counts from here (#716)
        self.begin_load_count();
        let read =
            flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::SceneRead, || {
                target_scene.to_string()
            });
        let next = self.read_scene(target_scene);
        drop(read);
        let next = match next {
            Ok(next) => next,
            Err(e) => {
                tracing::error!(target: "transition", "{e}; staying in {}", self.core.scene_path);
                if self.outcome.scene_failure.is_none() {
                    self.outcome.scene_failure = Some(e);
                }
                // Nothing was loaded: no table for it
                flint_core::load_report::abandon();
                return false;
            }
        };

        // Teardown: scripts told, subsystems let go (the music's stems stop
        // BEFORE audio.clear(), ADR 0017's producer→handles→device order),
        // systems/world/terrain/transient render state cleared.
        flint_core::frame_notes::timed(
            flint_core::frame_notes::NoteKind::SceneTeardown,
            String::new,
            || self.teardown_current_scene(),
        );

        // The outgoing scene's on_scene_exit has run: persist whatever it
        // (and the scene before it) stored, before the path changes.
        self.flush_persistent_store(true, "scene transition");

        self.install_scene(next);

        // The one bring-up startup uses too (`scene_entry.rs`)
        if let Err(e) = self.enter_scene(super::scene_entry::SceneEntry::Transition) {
            tracing::warn!(target: "transition", "{e:#}");
        }
        tracing::debug!(target: "transition", "Scene loaded: {}", self.core.scene_path);
        true
    }

    fn teardown_current_scene(&mut self) {
        tracing::debug!(target: "transition", "Unloading the current scene");

        // Call on_scene_exit on all scripts
        {
            let _state_scope = self.core.script.state_scope(
                &mut self.core.state_machine,
                &mut self.core.store,
                &self.core.physics,
            );
            self.core.script.call_scene_exits(&mut self.core.world);
        }
        // Each subsystem lets go of the scene, before anything is cleared:
        // the music's capture guard drops and its stems stop BEFORE the
        // audio system clears (ADR 0017's producer→handles→device order)
        self.subsystems_leave_scene();

        // A render mode a script held on: the next frame would otherwise
        // put the old scene's settings back over the new scene's
        self.post.mode_shim.forget();
        // A look's fade eases toward this scene's config, not the next's
        self.style.look_fade = None;
        // Its looks as read at its entry (#1159): the next scene reads its own
        self.style.preloaded_looks.clear();

        // Clear all systems
        self.core.script.clear();
        self.audio.clear();
        self.core.physics.clear();
        // The old scene's contacts name entities that are going; a script's
        // or an extension's own events go on to the next scene's scripts
        // (#727: they used to be lost with physics' queue).
        self.core.events.retain(|event| !event.is_contact());
        self.core.animation.clear();
        self.core.particles.clear();
        #[cfg(feature = "procgen")]
        self.core.procgen_resolver.clear_queue();

        // Clear world, keeping its ids: nothing spawned before the next
        // scene's world is installed reuses one
        self.core.world = FlintWorld::after(&self.core.world);

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
        // Style tweens name the old scene's entities (#370), as field
        // tweens do (#734)
        self.core.style_tweens.clear();
        self.core.field_tweens.clear();
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
        tracing::info!(target: "transition", scene = target_scene, "Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let path = resolve_scene_path(&self.core.project, &self.core.scene_path, target_scene);

        // The new scene's schema directories and its registry; kept as they
        // were when the scene doesn't load
        let schema_dirs = self.schema_dirs.clone();
        let registry = self.schemas_for_scene(&path);

        // Living props' tables are checked and their preset filled in, as at
        // the first load (#686): a scene that won't settle doesn't load.
        // Its ids start past the scene playing's (#688), so an id a script
        // kept (a persistent value, an event that outlives the scene) names
        // nothing in the next one, as it did under the old global counter
        let loaded = flint_scene::load_scene_after(&path, &registry, &self.core.world).and_then(
            |(world, mut file)| {
                #[cfg(feature = "living")]
                flint_procgen::settle_scene(&mut file, Path::new(&path))?;
                #[cfg(not(feature = "living"))]
                let _ = &mut file;
                Ok((world, file))
            },
        );
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
        self.core.world = next.world;
        self.core.scene_path = next.path;
        // The new scene may belong to another project (ADR 0114)
        self.refresh_project();
        // A slow-motion moment never outlives the scene that asked for it
        self.core.clock.time_scale = 1.0;
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
