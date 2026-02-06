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

use context::{InputSnapshot, ScriptCommand};
use engine::ScriptEngine;
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::{GameEvent, InputState, RuntimeSystem};
use sync::ScriptSync;

/// Top-level scripting system integrating engine, sync, and the game loop
pub struct ScriptSystem {
    pub engine: ScriptEngine,
    pub sync: ScriptSync,
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
        let snapshot = InputSnapshot {
            actions_pressed: snapshot_actions(input, true),
            actions_just_pressed: snapshot_actions(input, false),
            mouse_delta: input.raw_mouse_delta(),
        };

        self.engine.provide_context(snapshot, delta_time, total_time);
        self.pending_events = events.to_vec();
    }

    /// Drain commands produced by scripts this frame
    pub fn drain_commands(&mut self) -> Vec<ScriptCommand> {
        self.engine.drain_commands()
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

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        // Check for hot-reloaded scripts
        self.sync.check_hot_reload(&mut self.engine);

        // Discover any new script entities
        self.sync.discover_and_load(world, &mut self.engine);

        // Call on_init for newly added scripts
        self.engine.call_inits(world);

        // Process events (collision, trigger, action callbacks)
        let events = std::mem::take(&mut self.pending_events);
        self.engine.process_events(&events, world);

        // Call on_update(dt) for all scripts
        self.engine.call_updates(world, dt);

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

/// Snapshot which actions are pressed/just-pressed from InputState
fn snapshot_actions(input: &InputState, pressed: bool) -> std::collections::HashSet<String> {
    // Check known action names
    let action_names = [
        "move_forward", "move_backward", "move_left", "move_right",
        "jump", "interact", "sprint",
    ];

    let mut set = std::collections::HashSet::new();
    for action in &action_names {
        let active = if pressed {
            input.is_action_pressed(action)
        } else {
            input.is_action_just_pressed(action)
        };
        if active {
            set.insert(action.to_string());
        }
    }
    set
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
