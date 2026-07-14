//! Flint Script — Rhai scripting system
//!
//! Integrates the Rhai scripting engine into the Flint game loop:
//! - `ScriptEngine` — compiles and runs .rhai scripts, manages per-entity state
//! - `ScriptSync` — discovers entities with `script` component, handles hot-reload
//! - `ScriptSystem` — implements `RuntimeSystem` for game loop integration
//!
//! Scripts can read/write ECS data, respond to collisions/triggers/input,
//! control animation and audio, and hot-reload while the game is running.

pub mod api;
pub mod context;
pub mod engine;
pub mod sync;
pub mod ui;

pub use context::DrawCommand;
pub use context::StateScope;
use context::{InputSnapshot, ScriptCommand};
use engine::ScriptEngine;
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::{GameEvent, InputState, RuntimeSystem};
use sync::ScriptSync;

/// Top-level scripting system integrating engine, sync, and the game loop
pub struct ScriptSystem {
    pub(crate) engine: ScriptEngine,
    pub(crate) sync: ScriptSync,
    pending_events: Vec<GameEvent>,
}

impl ScriptSystem {
    pub fn new() -> Self {
        Self {
            engine: ScriptEngine::new(),
            sync: ScriptSync::new(),
            pending_events: Vec::new(),
        }
    }

    /// Clear all script state for a scene transition.
    /// Preserves the Rhai Engine and registered API functions.
    pub fn clear(&mut self) {
        self.engine.clear();
        self.sync.clear();
        self.pending_events.clear();
    }

    /// Provide input and timing context for the current frame.
    /// Called by PlayerApp before update().
    pub fn provide_context(
        &mut self,
        input: &InputState,
        events: &[GameEvent],
        total_time: f64,
        delta_time: f64,
    ) {
        // Snapshot input state
        let touches: Vec<(i64, f64, f64)> = input
            .active_touches()
            .values()
            .map(|t| (t.id as i64, t.position.0, t.position.1))
            .collect();
        let touch_just_started: Vec<(i64, f64, f64)> = input
            .touches_just_started_set()
            .iter()
            .filter_map(|id| input.touch_position(*id).map(|(x, y)| (*id as i64, x, y)))
            .collect();
        let touch_just_ended: Vec<i64> = input
            .touches_just_ended_set()
            .iter()
            .map(|id| *id as i64)
            .collect();
        let touch_taps: Vec<(f64, f64)> = input.touch_taps().to_vec();
        let touch_swipes: Vec<(String, f64, f64)> = input
            .touch_swipes()
            .iter()
            .map(|(dir, x, y)| (format!("{dir:?}").to_lowercase(), *x, *y))
            .collect();

        let snapshot = InputSnapshot {
            actions_pressed: snapshot_actions(input, true),
            actions_just_pressed: snapshot_actions(input, false),
            actions_just_released: snapshot_actions_released(input),
            action_values: snapshot_action_values(input),
            mouse_delta: input.raw_mouse_delta(),
            touches,
            touch_just_started,
            touch_just_ended,
            touch_taps,
            touch_swipes,
        };

        self.engine
            .provide_context(snapshot, delta_time, total_time);
        self.pending_events = events.to_vec();
    }

    /// Create an RAII guard that sets state_machine, persistent_store, and physics
    /// pointers for the duration of a scope. All three are cleared on Drop.
    pub fn state_scope(
        &mut self,
        sm: &mut flint_runtime::GameStateMachine,
        store: &mut flint_runtime::PersistentStore,
        physics: &flint_physics::PhysicsSystem,
    ) -> StateScope {
        StateScope::new(&self.engine.ctx, sm, store, physics)
    }

    /// Set transition state for script access
    pub fn set_transition_state(&mut self, progress: f64, phase: &str) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.transition_progress = progress;
        c.transition_phase = phase.to_string();
    }

    /// Set the current scene path for script access
    pub fn set_current_scene(&mut self, path: &str) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.current_scene_path = path.to_string();
    }

    /// Set camera position and direction for weapon aiming
    pub fn set_camera(&mut self, position: [f32; 3], direction: [f32; 3]) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.camera_position = position;
        c.camera_direction = direction;
    }

    /// Drain commands produced by scripts this frame
    pub fn drain_commands(&mut self) -> Vec<ScriptCommand> {
        self.engine.drain_commands()
    }

    /// Set screen dimensions for UI draw functions
    pub fn set_screen_size(&mut self, w: f32, h: f32) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.screen_width = w;
        c.screen_height = h;
    }

    /// Sync loaded chunk IDs so scripts can query `is_chunk_loaded`
    pub fn set_loaded_chunk_ids(&mut self, ids: std::collections::HashSet<String>) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.loaded_chunk_ids = ids;
    }

    /// Load a script file for a specific entity (for chunk loading).
    /// The entity must already have a `script` component in the ECS.
    pub fn load_script_for_entity(
        &mut self,
        entity_id: flint_core::EntityId,
        script_path: &std::path::Path,
    ) {
        match self.engine.compile_file(script_path) {
            Ok(ast) => {
                let source = script_path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("unknown.rhai")
                    .to_string();
                println!("[script] Loaded (chunk): {:?} → {}", entity_id, source);
                self.engine.add_script(entity_id, ast, source);
                self.sync.discovered.insert(entity_id);
            }
            Err(e) => {
                tracing::warn!(
                    "Compile error for chunk entity {:?}: {}",
                    entity_id, e
                );
            }
        }
    }

    /// Initialize scripts for specific entities (call on_init).
    pub fn initialize_entities(
        &mut self,
        world: &mut FlintWorld,
        entity_ids: &[flint_core::EntityId],
    ) {
        self.engine.call_inits_for(world, entity_ids);
    }

    /// Remove script state for an entity (for chunk unloading).
    pub fn remove_entity(&mut self, entity_id: flint_core::EntityId) {
        self.engine.scripts.remove(&entity_id);
        self.sync.discovered.remove(&entity_id);
    }

    /// Call on_draw_ui() for all scripts
    pub fn call_draw_uis(&mut self, world: &mut FlintWorld) {
        self.engine.call_draw_uis(world);
    }

    /// Drain draw commands produced by scripts this frame
    pub fn drain_draw_commands(&mut self) -> Vec<DrawCommand> {
        self.engine.drain_draw_commands()
    }

    /// Generate draw commands from the data-driven UI system
    pub fn generate_ui_draw_commands(&mut self, screen_w: f32, screen_h: f32) -> Vec<DrawCommand> {
        let mut c = self.engine.ctx.lock().unwrap();
        c.ui_system.generate_draw_commands(screen_w, screen_h)
    }

    /// Call on_animation_end(clip_name) for entities whose sprite animation finished
    pub fn call_sprite_anim_ends(
        &mut self,
        world: &mut FlintWorld,
        events: &[flint_animation::sprite_sync::SpriteAnimEndEvent],
    ) {
        self.engine.call_sprite_anim_ends(world, events);
    }

    /// Call on_scene_exit() for all scripts that define it
    pub fn call_scene_exits(&mut self, world: &mut FlintWorld) {
        self.engine.call_scene_exits(world);
    }

    /// Call on_scene_enter() for all scripts that define it
    pub fn call_scene_enters(&mut self, world: &mut FlintWorld) {
        self.engine.call_scene_enters(world);
    }

    /// Take camera overrides set by scripts this frame (clears them)
    pub fn take_camera_overrides(
        &mut self,
    ) -> (
        Option<[f32; 3]>,
        Option<[f32; 3]>,
        Option<f32>,
        Option<bool>,
        Option<f32>,
    ) {
        let mut c = self.engine.ctx.lock().unwrap();
        let pos = c.camera_position_override.take();
        let target = c.camera_target_override.take();
        let fov = c.camera_fov_override.take();
        let orthographic = c.camera_orthographic_override.take();
        let ortho_height = c.camera_ortho_height_override.take();
        (pos, target, fov, orthographic, ortho_height)
    }

    /// Take post-processing overrides set by scripts this frame (clears them)
    pub fn take_postprocess_overrides(
        &mut self,
    ) -> (
        Option<f32>,
        Option<f32>,
        Option<f32>,
        Option<f32>,
        Option<f32>,
        Option<f32>,
        Option<f32>,
    ) {
        let mut c = self.engine.ctx.lock().unwrap();
        let vignette = c.postprocess_vignette_override.take();
        let bloom = c.postprocess_bloom_override.take();
        let exposure = c.postprocess_exposure_override.take();
        let chromatic_aberration = c.postprocess_chromatic_aberration_override.take();
        let radial_blur = c.postprocess_radial_blur_override.take();
        let ssao_intensity = c.postprocess_ssao_intensity_override.take();
        let fog_density = c.postprocess_fog_density_override.take();
        (
            vignette,
            bloom,
            exposure,
            chromatic_aberration,
            radial_blur,
            ssao_intensity,
            fog_density,
        )
    }

    /// Take audio low-pass filter override set by scripts this frame (clears it)
    pub fn take_audio_overrides(&mut self) -> Option<f32> {
        let mut c = self.engine.ctx.lock().unwrap();
        c.audio_lowpass_cutoff_override.take()
    }

    /// Take the cursor capture request set by scripts this frame (clears it)
    pub fn take_cursor_capture_override(&mut self) -> Option<bool> {
        let mut c = self.engine.ctx.lock().unwrap();
        c.cursor_captured_override.take()
    }

    /// Set up the script system's scripts directory from the scene path
    pub fn load_scripts_from_scene(&mut self, scene_path: &str) {
        sync::load_scripts_from_scene(scene_path, &mut self.sync);
    }

    /// Set the terrain height sampling callback for scripts
    pub fn set_terrain_height_fn(&mut self, f: Option<Box<dyn Fn(f32, f32) -> f32 + Send + Sync>>) {
        let mut c = self.engine.ctx.lock().unwrap();
        c.terrain_height_fn = f;
    }
}

impl Default for ScriptSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSystem for ScriptSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        // Discover scripts from world
        self.sync.discover_and_load(world, &mut self.engine);

        // Call on_init() for all loaded scripts
        self.engine.call_inits(world);

        let count = self.engine.scripts.len();
        if count > 0 {
            println!("Script system initialized ({} scripts)", count);
        }

        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Scripts run at variable rate (like animation), not fixed timestep
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Check for hot-reloaded scripts
        self.sync.check_hot_reload(&mut self.engine);

        // Discover any new script entities
        self.sync.discover_and_load(world, &mut self.engine);

        // Call on_init for newly added scripts
        self.engine.call_inits(world);

        // Process events (collision, trigger, action callbacks)
        let events = std::mem::take(&mut self.pending_events);
        self.engine.process_events(&events, world);

        // Call on_update() for all scripts
        self.engine.call_updates(world);

        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        let count = self.engine.scripts.len();
        if count > 0 {
            println!("Script system shut down ({} scripts)", count);
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "script"
    }
}

/// Snapshot which actions are pressed/just-pressed from InputState.
/// Dynamically iterates all registered action names instead of a hard-coded list.
fn snapshot_actions(input: &InputState, pressed: bool) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for action in input.action_names() {
        let active = if pressed {
            input.is_action_pressed(&action)
        } else {
            input.is_action_just_pressed(&action)
        };
        if active {
            set.insert(action);
        }
    }
    set
}

fn snapshot_actions_released(input: &InputState) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for action in input.actions_just_released() {
        set.insert(action);
    }
    set
}

fn snapshot_action_values(input: &InputState) -> std::collections::HashMap<String, f64> {
    let mut map = std::collections::HashMap::new();
    for action in input.action_names() {
        let val = input.action_value(&action);
        if val.abs() > 0.001 {
            map.insert(action, val as f64);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_system_lifecycle() {
        let mut system = ScriptSystem::new();
        let mut world = FlintWorld::new();

        let result = system.initialize(&mut world);
        assert!(result.is_ok());

        let result = system.update(&mut world, 0.016);
        assert!(result.is_ok());

        let result = system.shutdown();
        assert!(result.is_ok());

        assert_eq!(system.name(), "script");
    }

    #[test]
    fn test_drain_commands_empty() {
        let system = ScriptSystem::new();
        let commands = system.engine.drain_commands();
        assert!(commands.is_empty());
    }
}
