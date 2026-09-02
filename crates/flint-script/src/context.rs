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
use std::sync::{Arc, Mutex};

/// Persistent state for camera follow (survives across frames, reset on scene transition)
pub struct CameraFollowState {
    pub current_x: f32,
    pub current_y: f32,
    pub initialized: bool,
}

impl Default for CameraFollowState {
    fn default() -> Self {
        Self {
            current_x: 0.0,
            current_y: 0.0,
            initialized: false,
        }
    }
}

/// Persistent state for screen shake effect
pub struct ShakeState {
    pub amplitude: f32,
    pub frequency: f32,
    pub decay: f32,
    pub phase: f32,
}

impl Default for ShakeState {
    fn default() -> Self {
        Self {
            amplitude: 0.0,
            frequency: 20.0,
            decay: 6.0,
            phase: 0.0,
        }
    }
}

/// Snapshot of input state for script access (no winit dependency needed)
#[derive(Clone, Default)]
pub struct InputSnapshot {
    pub actions_pressed: HashSet<String>,
    pub actions_just_pressed: HashSet<String>,
    pub actions_just_released: HashSet<String>,
    pub action_values: std::collections::HashMap<String, f64>,
    /// Any raw key/mouse/gamepad press this frame (unbound keys included)
    pub any_just_pressed: bool,
    pub mouse_delta: (f64, f64),
    /// Active touches: (id, norm_x, norm_y)
    pub touches: Vec<(i64, f64, f64)>,
    /// Touches that just started this frame: (id, norm_x, norm_y)
    pub touch_just_started: Vec<(i64, f64, f64)>,
    /// Touch IDs that just ended this frame
    pub touch_just_ended: Vec<i64>,
    /// Tap positions detected this frame (norm_x, norm_y)
    pub touch_taps: Vec<(f64, f64)>,
    /// Swipe gestures detected this frame (direction_str, start_norm_x, start_norm_y)
    pub touch_swipes: Vec<(String, f64, f64)>,
}

/// One judged pulse event from the music session, for scripts. A plain POD
/// mirror — flint-script never depends on flint-music (ADR 0020).
#[derive(Clone, Debug, PartialEq)]
pub struct ConductedPulse {
    /// Seconds before "now" the event was stamped (suite-time arithmetic).
    pub age: f64,
    /// Timing error in ms (hits only; 0.0 for miss/spurious).
    pub err_ms: f64,
    /// "hit" | "miss" | "spurious".
    pub kind: String,
}

/// One typed value from a chart cue's free-form `params` table (ADR 0033).
/// A plain POD — flint-script never depends on flint-music or toml shapes.
#[derive(Clone, Debug, PartialEq)]
pub enum CueParam {
    Number(f64),
    Text(String),
    Flag(bool),
}

/// One chart cue fired this frame (ADR 0033), for scene bindings.
#[derive(Clone, Debug, PartialEq)]
pub struct ConductedCue {
    pub name: String,
    /// Seconds since the cue's authored beat (suite-time arithmetic).
    pub age: f64,
    /// The chart's `params` table, flattened to primitives (nested tables
    /// and arrays are dropped by the host — author flat params).
    pub params: Vec<(String, CueParam)>,
}

/// Per-frame conducted-parameters snapshot (Phase 4 decision 4 / ADR 0020).
/// Filled by the host (flint-player) from the music session every frame;
/// neutral defaults when no session so scripts never branch on existence.
#[derive(Clone, Debug, PartialEq)]
pub struct ConductedSnapshot {
    /// Player lean and the chart's lean target, both in [-1,1]².
    pub lean: [f64; 2],
    pub target: [f64; 2],
    /// Player sway (right stick; zeros under the prototype input map).
    pub sway: [f64; 2],
    /// Trigger depths in [0,1] (zeros under the prototype input map).
    pub pressure_l: f64,
    pub pressure_r: f64,
    /// Cues fired this frame (empty most frames — ADR 0033).
    pub cues: Vec<ConductedCue>,
    /// The next authored lean key's value (ADR 0023); `target` when nothing
    /// is upcoming.
    pub next_target: [f64; 2],
    /// Suite beats until that key's anchor; 1e6 = nothing upcoming.
    pub next_target_beats: f64,
    pub coherence: f64,
    /// 0..1 within the current beat / bar (0 at the boundary).
    pub beat_phase: f64,
    pub bar_phase: f64,
    pub bar: i64,
    /// Suite beats from zero, accumulated across tempo changes.
    pub beat: f64,
    /// Current section name; "" when none.
    pub section: String,
    /// Pulses judged this frame (empty most frames).
    pub pulses: Vec<ConductedPulse>,
    /// Ladder visual params (0 = clean).
    pub desaturate: f64,
    pub blur: f64,
    pub chromatic: f64,
    /// Reassembly progress: 1 = normal play, 0→1 while re-gathering.
    pub reassembly: f64,
    /// Rewind interlude progress: 0 = not rewinding.
    pub rewind: f64,
    pub no_input: bool,
    pub preroll: bool,
}

impl Default for ConductedSnapshot {
    /// The neutral no-session state: a clean, settled world (coherence and
    /// reassembly at 1.0) so bindings mapping `1 - coherence` show nothing.
    fn default() -> Self {
        Self {
            lean: [0.0; 2],
            target: [0.0; 2],
            sway: [0.0; 2],
            pressure_l: 0.0,
            pressure_r: 0.0,
            cues: Vec::new(),
            next_target: [0.0; 2],
            next_target_beats: 1e6,
            coherence: 1.0,
            beat_phase: 0.0,
            bar_phase: 0.0,
            bar: 0,
            beat: 0.0,
            section: String::new(),
            pulses: Vec::new(),
            desaturate: 0.0,
            blur: 0.0,
            chromatic: 0.0,
            reassembly: 1.0,
            rewind: 0.0,
            no_input: false,
            preroll: false,
        }
    }
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
    PlaySound {
        name: String,
        volume: f64,
    },
    PlaySoundAt {
        name: String,
        position: (f64, f64, f64),
        volume: f64,
        pitch: f64,
    },
    StopSound {
        name: String,
    },
    FireEvent {
        name: String,
        data: toml::Value,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    EmitBurst {
        entity_id: i64,
        count: i64,
    },
    LoadScene {
        path: String,
    },
    ReloadScene,
    PushState {
        name: String,
    },
    PopState,
    ReplaceState {
        name: String,
    },
    SetVelocity2D {
        entity_id: i64,
        vx: f64,
        vy: f64,
    },
    LoadChunk {
        path: String,
        offset_x: f64,
        offset_y: f64,
        chunk_id: String,
    },
    UnloadChunk {
        chunk_id: String,
    },
}

/// 2D draw command issued by scripts each frame (immediate mode)
#[derive(Debug, Clone)]
pub enum DrawCommand {
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: [f32; 4],
        layer: i32,
        /// 0 = left (default), 1 = center, 2 = right
        align: u8,
        /// Optional stroke (outline): color + pixel width
        stroke: Option<([f32; 4], f32)>,
    },
    RectFilled {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        rounding: f32,
        layer: i32,
    },
    RectOutline {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    CircleFilled {
        x: f32,
        y: f32,
        radius: f32,
        color: [f32; 4],
        layer: i32,
    },
    CircleOutline {
        x: f32,
        y: f32,
        radius: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: [f32; 4],
        thickness: f32,
        layer: i32,
    },
    Sprite {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
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
    /// Script-driven camera orthographic override
    pub camera_orthographic_override: Option<bool>,
    /// Script-driven camera ortho_height override
    pub camera_ortho_height_override: Option<f32>,
    /// Script-driven camera roll override, radians about the view axis.
    /// One-frame like the others; the host resets the camera up vector when absent.
    pub camera_roll_override: Option<f32>,
    /// Script-driven post-processing overrides
    pub postprocess_vignette_override: Option<f32>,
    pub postprocess_bloom_override: Option<f32>,
    pub postprocess_exposure_override: Option<f32>,
    pub postprocess_chromatic_aberration_override: Option<f32>,
    pub postprocess_radial_blur_override: Option<f32>,
    pub postprocess_ssao_intensity_override: Option<f32>,
    pub postprocess_fog_density_override: Option<f32>,
    pub postprocess_desaturation_override: Option<f32>,
    pub postprocess_dof_strength_override: Option<f32>,
    pub postprocess_dof_focus_distance_override: Option<f32>,
    pub postprocess_dof_focus_range_override: Option<f32>,
    pub postprocess_fog_color_override: Option<[f32; 3]>,
    /// Reality-tear render mode override: (mode, mix). Set by set_render_mode.
    pub postprocess_render_mode_override: Option<(u32, f32)>,
    /// Per-mode tuning params. Set by set_render_mode_params.
    pub postprocess_mode_params_override: Option<[f32; 4]>,
    /// Script-driven audio low-pass filter override (cutoff frequency in Hz)
    pub audio_lowpass_cutoff_override: Option<f32>,
    /// Script-driven cursor capture request. Games without a character
    /// controller (which normally gates capture) use this for mouse look.
    pub cursor_captured_override: Option<bool>,
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
    /// Terrain height sampling callback — set by PlayerApp if terrain is loaded
    pub terrain_height_fn: Option<Box<dyn Fn(f32, f32) -> f32 + Send + Sync>>,
    /// Persistent camera follow state (survives across frames)
    pub camera_follow: CameraFollowState,
    /// Persistent screen shake state
    pub shake: ShakeState,
    /// Set of currently loaded chunk IDs (synced from PlayerApp before scripts run)
    pub loaded_chunk_ids: HashSet<String>,
    /// Conducted-parameters snapshot for this frame (set by the host before
    /// scripts run; neutral defaults when no music session — ADR 0020)
    pub conducted: ConductedSnapshot,
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
            camera_orthographic_override: None,
            camera_ortho_height_override: None,
            camera_roll_override: None,
            postprocess_vignette_override: None,
            postprocess_bloom_override: None,
            postprocess_exposure_override: None,
            postprocess_chromatic_aberration_override: None,
            postprocess_radial_blur_override: None,
            postprocess_ssao_intensity_override: None,
            postprocess_fog_density_override: None,
            postprocess_desaturation_override: None,
            postprocess_dof_strength_override: None,
            postprocess_dof_focus_distance_override: None,
            postprocess_dof_focus_range_override: None,
            postprocess_fog_color_override: None,
            postprocess_render_mode_override: None,
            postprocess_mode_params_override: None,
            audio_lowpass_cutoff_override: None,
            cursor_captured_override: None,
            state_machine: std::ptr::null_mut(),
            persistent_store: std::ptr::null_mut(),
            transition_progress: -1.0,
            transition_phase: String::from("idle"),
            current_scene_path: String::new(),
            ui_system: UiSystem::new(),
            terrain_height_fn: None,
            camera_follow: CameraFollowState::default(),
            shake: ShakeState::default(),
            loaded_chunk_ids: HashSet::new(),
            conducted: ConductedSnapshot::default(),
        }
    }

    /// Get a reference to the world. Panics if called outside a valid scope.
    ///
    /// # Safety
    /// Caller must ensure the world pointer was set and is still valid
    /// (i.e., called within the scope of `call_update` or `process_events`).
    pub unsafe fn world_ref(&self) -> &FlintWorld {
        assert!(
            !self.world.is_null(),
            "ScriptCallContext: world pointer is null (called outside scope)"
        );
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
        assert!(
            !self.world.is_null(),
            "ScriptCallContext: world pointer is null (called outside scope)"
        );
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

/// RAII guard: sets world pointer on construction, clears on Drop.
///
/// Clones the `Arc<Mutex<ScriptCallContext>>` so the mutex is NOT held between
/// construction and drop — callers can re-lock freely (e.g. to set `current_entity`).
pub struct WorldScope {
    ctx: Arc<Mutex<ScriptCallContext>>,
}

impl WorldScope {
    pub fn new(ctx: &Arc<Mutex<ScriptCallContext>>, world: &mut FlintWorld) -> Self {
        {
            let mut c = crate::lock_or_recover(&ctx);
            c.world = world as *mut FlintWorld;
        }
        Self { ctx: ctx.clone() }
    }
}

impl Drop for WorldScope {
    fn drop(&mut self) {
        let mut c = crate::lock_or_recover(&self.ctx);
        c.world = std::ptr::null_mut();
    }
}

/// RAII guard: sets state_machine, persistent_store, and physics pointers.
/// Clears all three on Drop.
pub struct StateScope {
    ctx: Arc<Mutex<ScriptCallContext>>,
}

impl StateScope {
    pub fn new(
        ctx: &Arc<Mutex<ScriptCallContext>>,
        state_machine: &mut GameStateMachine,
        persistent_store: &mut PersistentStore,
        physics: &PhysicsSystem,
    ) -> Self {
        {
            let mut c = crate::lock_or_recover(&ctx);
            c.state_machine = state_machine as *mut GameStateMachine;
            c.persistent_store = persistent_store as *mut PersistentStore;
            c.physics = physics as *const PhysicsSystem;
        }
        Self { ctx: ctx.clone() }
    }
}

impl Drop for StateScope {
    fn drop(&mut self) {
        let mut c = crate::lock_or_recover(&self.ctx);
        c.state_machine = std::ptr::null_mut();
        c.persistent_store = std::ptr::null_mut();
        c.physics = std::ptr::null();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_scope_sets_and_clears_pointer() {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut world = FlintWorld::new();

        // Pointer starts null
        assert!(ctx.lock().unwrap().world.is_null());

        {
            let _scope = WorldScope::new(&ctx, &mut world);
            // Pointer is set inside scope
            assert!(!ctx.lock().unwrap().world.is_null());
        }

        // Pointer cleared after scope drops
        assert!(ctx.lock().unwrap().world.is_null());
    }

    #[test]
    fn world_scope_clears_on_panic() {
        let ctx = Arc::new(Mutex::new(ScriptCallContext::new()));
        let mut world = FlintWorld::new();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = WorldScope::new(&ctx, &mut world);
            assert!(!ctx.lock().unwrap().world.is_null());
            panic!("intentional panic to test Drop");
        }));

        assert!(result.is_err());
        // Pointer still cleared despite panic
        assert!(ctx.lock().unwrap().world.is_null());
    }
}
