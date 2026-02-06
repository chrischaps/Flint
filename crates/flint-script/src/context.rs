//! Script execution context and command types
//!
//! ScriptCallContext is the shared state accessed by Rhai API functions during script execution.
//! ScriptCommand represents deferred actions (audio, events, logging) collected during a script call.

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashSet;

/// Snapshot of input state for script access (no winit dependency needed)
#[derive(Clone, Default)]
pub struct InputSnapshot {
    pub actions_pressed: HashSet<String>,
    pub actions_just_pressed: HashSet<String>,
    pub mouse_delta: (f64, f64),
}

/// Log severity levels
#[derive(Debug, Clone)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Deferred commands produced by scripts, processed by PlayerApp after script update
#[derive(Debug, Clone)]
pub enum ScriptCommand {
    PlaySound { name: String, volume: f64 },
    PlaySoundAt { name: String, position: (f64, f64, f64), volume: f64 },
    StopSound { name: String },
    FireEvent { name: String, data: toml::Value },
    Log { level: LogLevel, message: String },
}

/// Shared context set before each script call and read by registered Rhai functions.
///
/// Safety: the `world` pointer is only valid during the scope of `call_update` /
/// `process_events`. It is set to null immediately after each call batch.
pub struct ScriptCallContext {
    /// Raw pointer to the FlintWorld — valid only during call scope
    pub world: *mut FlintWorld,
    /// Entity currently being scripted
    pub current_entity: EntityId,
    /// Accumulated commands to be drained after all scripts run
    pub commands: Vec<ScriptCommand>,
    /// Input snapshot for this frame
    pub input: InputSnapshot,
    /// Frame delta time
    pub delta_time: f64,
    /// Total elapsed game time
    pub total_time: f64,
}

// SAFETY: ScriptCallContext is only accessed from the main thread within
// controlled call scopes. The world pointer is valid only during those scopes.
unsafe impl Send for ScriptCallContext {}
unsafe impl Sync for ScriptCallContext {}

impl Default for ScriptCallContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptCallContext {
    pub fn new() -> Self {
        Self {
            world: std::ptr::null_mut(),
            current_entity: EntityId::from_raw(0),
            commands: Vec::new(),
            input: InputSnapshot::default(),
            delta_time: 0.0,
            total_time: 0.0,
        }
    }

    /// Get a reference to the world. Panics if called outside a valid scope.
    ///
    /// # Safety
    /// Caller must ensure the world pointer was set and is still valid
    /// (i.e., called within the scope of `call_update` or `process_events`).
    pub unsafe fn world_ref(&self) -> &FlintWorld {
        assert!(!self.world.is_null(), "ScriptCallContext: world pointer is null (called outside scope)");
        unsafe { &*self.world }
    }

    /// Get a mutable reference to the world. Panics if called outside a valid scope.
    ///
    /// # Safety
    /// Caller must ensure the world pointer was set, is still valid, and no other
    /// references to the world exist (i.e., called within the scope of
    /// `call_update` or `process_events`).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn world_mut(&self) -> &mut FlintWorld {
        assert!(!self.world.is_null(), "ScriptCallContext: world pointer is null (called outside scope)");
        unsafe { &mut *self.world }
    }
}
