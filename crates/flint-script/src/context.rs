//! Script execution context and command types
//!
//! ScriptCallContext is the shared state accessed by Rhai API functions during script execution.
//! ScriptCommand represents deferred actions (audio, events, logging) collected during a script call.

use crate::ui::UiSystem;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use flint_runtime::{GameStateMachine, PersistentStore};
use std::collections::HashSet;

/// Snapshot of input state for script access (no winit dependency needed)
#[derive(Clone, Default)]
pub struct InputSnapshot {
    pub actions_pressed: HashSet<String>,
    pub actions_just_pressed: HashSet<String>,
    pub actions_just_released: HashSet<String>,
    pub action_values: std::collections::HashMap<String, f64>,
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
    EmitBurst { entity_id: i64, count: i64 },
    LoadScene { path: String },
    ReloadScene,
    PushState { name: String },
    PopState,
    ReplaceState { name: String },
}

/// 2D draw command issued by scripts each frame (immediate mode)
#[derive(Debug, Clone)]
pub enum DrawCommand {
    Text {
        x: f32, y: f32,
        text: String,
        size: f32,
        color: [f32; 4],
        layer: i32,
    },
    RectFilled {
        x: f32, y: f32, w: f32, h: f32,
        color: [f32; 4],
        rounding: f32,
        layer: i32,
    },
    RectOutline {
        x: f32, y: f32, w: f32, h: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    CircleFilled {
        x: f32, y: f32,
        radius: f32,
        color: [f32; 4],
        layer: i32,
    },
    CircleOutline {
        x: f32, y: f32,
        radius: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    Line {
        x1: f32, y1: f32, x2: f32, y2: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    Sprite {
        x: f32, y: f32, w: f32, h: f32,
        name: String,
        uv: [f32; 4],
        tint: [f32; 4],
        layer: i32,
    },
}

impl DrawCommand {
    pub fn layer(&self) -> i32 {
        match self {
            DrawCommand::Text { layer, .. } => *layer,
            DrawCommand::RectFilled { layer, .. } => *layer,
            DrawCommand::RectOutline { layer, .. } => *layer,
            DrawCommand::CircleFilled { layer, .. } => *layer,
            DrawCommand::CircleOutline { layer, .. } => *layer,
            DrawCommand::Line { layer, .. } => *layer,
            DrawCommand::Sprite { layer, .. } => *layer,
        }
    }
}

/// Shared context set before each script call and read by registered Rhai functions.
///
/// Safety: the `world` pointer is only valid during the scope of `call_update` /
/// `process_events`. It is set to null immediately after each call batch.
pub struct ScriptCallContext {
    /// Raw pointer to the FlintWorld — valid only during call scope
    pub world: *mut FlintWorld,
    /// Raw pointer to the PhysicsSystem — valid only during call scope
    pub physics: *const PhysicsSystem,
    /// Camera position and direction for weapon aiming
    pub camera_position: [f32; 3],
    pub camera_direction: [f32; 3],
    /// Entity currently being scripted
    pub current_entity: EntityId,
    /// Accumulated commands to be drained after all scripts run
    pub commands: Vec<ScriptCommand>,
    /// Accumulated 2D draw commands for the current frame
    pub draw_commands: Vec<DrawCommand>,
    /// Input snapshot for this frame
    pub input: InputSnapshot,
    /// Frame delta time
    pub delta_time: f64,
    /// Total elapsed game time
    pub total_time: f64,
    /// Screen dimensions in pixels (set before scripts run)
    pub screen_width: f32,
    pub screen_height: f32,
    /// Script-driven camera overrides (set by set_camera_position/set_camera_target/set_camera_fov)
    pub camera_position_override: Option<[f32; 3]>,
    pub camera_target_override: Option<[f32; 3]>,
    pub camera_fov_override: Option<f32>,
    /// Script-driven post-processing overrides
    pub postprocess_vignette_override: Option<f32>,
    pub postprocess_bloom_override: Option<f32>,
    pub postprocess_exposure_override: Option<f32>,
    pub postprocess_chromatic_aberration_override: Option<f32>,
    pub postprocess_radial_blur_override: Option<f32>,
    pub postprocess_ssao_intensity_override: Option<f32>,
    /// Script-driven audio low-pass filter override (cutoff frequency in Hz)
    pub audio_lowpass_cutoff_override: Option<f32>,
    /// Raw pointer to the GameStateMachine — valid only during call scope
    pub state_machine: *mut GameStateMachine,
    /// Raw pointer to the PersistentStore — valid only during call scope
    pub persistent_store: *mut PersistentStore,
    /// Transition progress (0.0-1.0 during transitions, -1.0 when idle)
    pub transition_progress: f64,
    /// Current transition phase name ("idle", "exiting", "entering")
    pub transition_phase: String,
    /// Path of the currently loaded scene
    pub current_scene_path: String,
    /// Data-driven UI system
    pub ui_system: UiSystem,
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
            physics: std::ptr::null(),
            camera_position: [0.0; 3],
            camera_direction: [0.0, 0.0, 1.0],
            current_entity: EntityId::from_raw(0),
            commands: Vec::new(),
            draw_commands: Vec::new(),
            input: InputSnapshot::default(),
            delta_time: 0.0,
            total_time: 0.0,
            screen_width: 1280.0,
            screen_height: 720.0,
            camera_position_override: None,
            camera_target_override: None,
            camera_fov_override: None,
            postprocess_vignette_override: None,
            postprocess_bloom_override: None,
            postprocess_exposure_override: None,
            postprocess_chromatic_aberration_override: None,
            postprocess_radial_blur_override: None,
            postprocess_ssao_intensity_override: None,
            audio_lowpass_cutoff_override: None,
            state_machine: std::ptr::null_mut(),
            persistent_store: std::ptr::null_mut(),
            transition_progress: -1.0,
            transition_phase: String::from("idle"),
            current_scene_path: String::new(),
            ui_system: UiSystem::new(),
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

    /// Get a reference to the physics system. Returns None if not set.
    ///
    /// # Safety
    /// Caller must ensure the physics pointer was set and is still valid.
    pub unsafe fn physics_ref(&self) -> Option<&PhysicsSystem> {
        if self.physics.is_null() {
            None
        } else {
            Some(unsafe { &*self.physics })
        }
    }
}
