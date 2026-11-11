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
    /// Device of the most recent deliberate input: "keyboard" | "mouse" |
    /// "gamepad" | "touch" (empty only before the first snapshot).
    pub last_device: String,
    /// True while the host reports at least one gamepad attached.
    pub gamepad_connected: bool,
    /// True while the on-screen touch controls are drawn (#133).
    pub touch_controls_visible: bool,
    /// True on a touch-first device (`pointer: coarse` page, Android) from
    /// the first frame, whatever has been touched so far (#660).
    pub touch_device: bool,
    /// Safe-area insets in points: left, top, right, bottom (#660).
    pub safe_area: [f64; 4],
    pub mouse_delta: (f64, f64),
    /// Wheel lines (notches) this frame, (x, y); up and right are positive.
    pub mouse_wheel: (f64, f64),
    /// Mouse buttons currently held (0 left, 1 right, 2 middle)
    pub mouse_buttons_down: Vec<i64>,
    /// Mouse buttons pressed this frame
    pub mouse_buttons_just_pressed: Vec<i64>,
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
    /// Suite beats until the next unconsumed judgment window's anchor beat
    /// (any kind); 1e6 = nothing upcoming. Lets the world telegraph that
    /// input is expected (P5 immediate-feedback pass).
    pub next_pulse_beats: f64,
    /// True while that window is open right now (a press would land).
    pub pulse_window_open: bool,
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
            next_pulse_beats: 1e6,
            pulse_window_open: false,
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
        pitch: f64,
    },
    PlaySoundAt {
        name: String,
        position: (f64, f64, f64),
        volume: f64,
        pitch: f64,
        /// `(min, max)` metres from the listener; `None` is the engine's
        /// default (1 to 25).
        distances: Option<(f32, f32)>,
        /// The falloff curve between them; `None` is the engine's default.
        rolloff: Option<f32>,
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
    /// Spawn a detached one-shot instance of a registered particle effect
    /// (ADR 0068). `handle` is the id `play_effect` returned to the script.
    PlayEffect {
        handle: u64,
        name: String,
        position: (f64, f64, f64),
    },
    /// Stop emission on a detached effect; particles in flight finish.
    StopEffect {
        handle: u64,
    },
    /// Tune a detached effect: `emission_scale`, `scale`, `playing`, `x`/`y`/`z`.
    SetEffectParam {
        handle: u64,
        param: String,
        value: f64,
    },
    /// Draw an entity (and its descendants) in a per-entity style such as
    /// `ascii` (#269). The player adds the style's component.
    SetEntityStyle {
        entity_id: i64,
        style: String,
    },
    /// Change one setting of an entity's style: a parameter (number), an
    /// option (string), `enabled` (bool) or a 3-array such as `tint`.
    SetStyleParam {
        entity_id: i64,
        param: String,
        value: toml::Value,
    },
    /// Ease one setting of an entity's style (a number, `blend` or a
    /// colour) to `target` over `seconds` of game time (#370). The player
    /// writes the field each frame and fires `style_tween_done` at the end;
    /// a new tween of the same setting replaces the old one.
    TweenStyleParam {
        entity_id: i64,
        param: String,
        target: toml::Value,
        seconds: f64,
        /// `linear`, `smooth` (the default), `ease_in`, `ease_out` or
        /// `ease_in_out`
        easing: String,
    },
    /// Take an entity's own style off again.
    ClearEntityStyle {
        entity_id: i64,
    },
    /// Switch a style on or off for the whole frame (#369): its
    /// `[post_process.styles.<name>]` table, any registered style.
    SetPostStyle {
        style: String,
        enabled: bool,
    },
    /// Change one setting of a whole-frame style: a parameter (number), an
    /// option (string), a colour, `enabled`, `blend` (0..1), `order`.
    SetPostStyleParam {
        style: String,
        param: String,
        value: toml::Value,
    },
    /// Put a whole-frame style back to what the scene authored.
    ResetPostStyle {
        style: String,
    },
    /// The picture no longer follows on from the last frame (a camera cut,
    /// #343): styles with frame-to-frame state (ASCII's cell history, #277)
    /// draw the next frame as if it were the first.
    ResetStyleHistory,
    /// Turn the player's own camera-cut detection on or off (#343). On by
    /// default: a jump or turn far past the camera's recent motion resets
    /// style history by itself.
    SetCameraCutDetection {
        enabled: bool,
    },
    /// Lay a look under the scene's `[post_process]` keys in place of the
    /// one it has (#356): a `styles/<name>.look.toml` or a built-in.
    /// `None` takes the look off (the scene's keys alone).
    SetLook {
        name: Option<String>,
    },
    /// Change the wooze field's global knobs (#213): any of `amplitude`, `step`,
    /// `glide`, `jitter` and `preset`, over `seconds`.
    SetWooze {
        knobs: toml::Value,
        seconds: f64,
    },
    /// Change a wooze zone, or add it if there's none of that name, over `seconds`.
    SetWoozeZone {
        name: String,
        fields: toml::Value,
        seconds: f64,
    },
    /// Fade a wooze zone out over `seconds` and drop it.
    RemoveWoozeZone {
        name: String,
        seconds: f64,
    },
    /// Turn Living Props' knobs (#218): a table shaped like a `.wooze.toml`, with
    /// any of `wooze`, `living` and `boil`.
    TuneLiving {
        knobs: toml::Value,
    },
    /// Clear one species' own Living Props values (#246), so the layers below
    /// decide them again: the named keys, or every one with none named.
    ClearLiving {
        species: String,
        keys: Vec<String>,
    },
    /// Stop or start the clock living props run on.
    FreezeLiving {
        frozen: bool,
    },
    /// Move that clock on by exactly one beat.
    StepLiving,
    /// Save every Living Props knob as `wooze/<name>.wooze.toml`.
    SaveLivingPreset {
        name: String,
    },
    /// Load a preset file or a built-in, live.
    LoadLivingPreset {
        name: String,
    },
    /// Write every Living Props value the scene file doesn't give into it
    /// (#249), as the F3 panel's Commit to File does.
    CommitLiving,
    /// Compare two presets on the same grove (#238). An empty name is the knobs
    /// as they stand.
    CompareLiving {
        a: String,
        b: String,
    },
    /// Show the other side of the comparison.
    FlipLiving,
    /// Show both sides at once, A left and B right, or one at a time.
    SplitLiving {
        split: bool,
    },
    /// End the comparison, keeping the side that's shown.
    StopComparingLiving,
    LoadScene {
        path: String,
    },
    ReloadScene,
    /// End the run, as closing the window does (#659): natively the player
    /// flushes its saves and exits with code 0; on the web the page lets go
    /// of full screen and the pointer and goes back to its doorway, if any.
    QuitGame,
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

/// Host-installed text measurement: `(text, size, font family, letter spacing)`
/// → `(width, height)` in logical points.
pub type TextMeasurer = Arc<dyn Fn(&str, f32, Option<&str>, f32) -> (f32, f32) + Send + Sync>;

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
        /// Optional stroke (outline): colour + width in points. Up to 2 pt it's
        /// the text at eight offsets; wider, a round band (#626)
        stroke: Option<([f32; 4], f32)>,
        /// Font family name (a file stem or manifest alias under `<project>/fonts/`);
        /// `None` or an unknown name falls back to the default proportional font.
        font: Option<String>,
        /// Extra spacing between glyphs, in logical points (0 = font default).
        letter_spacing: f32,
        /// Optional drop shadow: (colour, dx, dy) drawn once beneath the text.
        shadow: Option<([f32; 4], f32, f32)>,
        /// Optional glow: colour + reach in points, a blur of the text and
        /// its stroke drawn beneath them (#626).
        glow: Option<([f32; 4], f32)>,
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
        rounding: f32,
        layer: i32,
    },
    /// Filled rectangle with per-corner rounding `[tl, tr, br, bl]`.
    RectRounded4 {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        rounding: [f32; 4],
        layer: i32,
    },
    /// Two-colour linear gradient fill: `color_a` → `color_b`, top→bottom
    /// when `vertical`, left→right otherwise.
    RectGradient {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color_a: [f32; 4],
        color_b: [f32; 4],
        vertical: bool,
        layer: i32,
    },
    /// Annular sector. Degrees from 12 o'clock, clockwise; `r_inner` may be 0.
    Ring {
        cx: f32,
        cy: f32,
        r_inner: f32,
        r_outer: f32,
        start_deg: f32,
        end_deg: f32,
        color: [f32; 4],
        layer: i32,
    },
    /// Convex filled polygon.
    Polygon {
        points: Vec<[f32; 2]>,
        color: [f32; 4],
        layer: i32,
    },
    /// Polyline, optionally closed.
    PolygonOutline {
        points: Vec<[f32; 2]>,
        color: [f32; 4],
        thickness: f32,
        closed: bool,
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
            DrawCommand::RectRounded4 { layer, .. } => *layer,
            DrawCommand::RectGradient { layer, .. } => *layer,
            DrawCommand::Ring { layer, .. } => *layer,
            DrawCommand::Polygon { layer, .. } => *layer,
            DrawCommand::PolygonOutline { layer, .. } => *layer,
            DrawCommand::CircleFilled { layer, .. } => *layer,
            DrawCommand::CircleOutline { layer, .. } => *layer,
            DrawCommand::Line { layer, .. } => *layer,
            DrawCommand::Sprite { layer, .. } => *layer,
        }
    }
}

/// A recorded draw command plus the clip rect (`[x, y, w, h]`, logical
/// points) that was active when it was issued — `None` = unclipped. The
/// clip is stamped per item so layer sorting stays a plain stable sort.
/// `over_ui` is stamped the same way: true when the item was issued inside
/// `push_over_ui` / `pop_over_ui`, and it then draws above the game's egui
/// (ADR 0085).
#[derive(Debug, Clone)]
pub struct DrawItem {
    pub cmd: DrawCommand,
    pub clip: Option<[f32; 4]>,
    pub over_ui: bool,
}

impl DrawItem {
    pub fn unclipped(cmd: DrawCommand) -> Self {
        Self {
            cmd,
            clip: None,
            over_ui: false,
        }
    }

    pub fn layer(&self) -> i32 {
        self.cmd.layer()
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
    /// View-projection of the camera the last frame was rendered with
    /// (column-major, wgpu clip space). Backs `draw_line_3d` / `world_to_screen`.
    pub camera_view_proj: [[f32; 4]; 4],
    /// Entity currently being scripted
    pub current_entity: EntityId,
    /// Accumulated commands to be drained after all scripts run
    pub commands: Vec<ScriptCommand>,
    /// Accumulated 2D draw commands for the current frame, each stamped
    /// with the clip rect active when it was issued
    pub draw_commands: Vec<DrawItem>,
    /// Depth-tested world lines queued this frame (`draw_world_line`,
    /// ADR 0083). The host hands them to the renderer.
    pub world_lines: Vec<flint_core::world_line::WorldLine>,
    /// Depth-tested world text queued this frame (`draw_world_text`,
    /// ADR 0104). The host lays it out and draws it before the HUD.
    pub world_texts: Vec<flint_core::world_text::WorldText>,
    /// `push_clip` / `pop_clip` stack; each entry is already intersected
    /// with its parent. Cleared when the frame's draw commands are drained.
    pub clip_stack: Vec<[f32; 4]>,
    /// `push_over_ui` / `pop_over_ui` depth; above zero, draw items are
    /// stamped `over_ui`. Reset with the clip stack each frame.
    pub over_ui_depth: u32,
    /// Mouse cursor in logical points (same space as `screen_width`);
    /// written by the host each frame alongside the screen size.
    pub mouse_pos_logical: (f32, f32),
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
    /// Time scale asked for by `set_time_scale`, applied to the game clock
    /// from the next frame and held until changed or a scene loads.
    pub time_scale_request: Option<f64>,
    /// Script-driven cursor capture request. Games without a character
    /// controller (which normally gates capture) use this for mouse look.
    pub cursor_captured_override: Option<bool>,
    /// `set_touch_controls(enabled)` this frame (#134).
    pub touch_controls_override: Option<bool>,
    /// `set_lod_scale(scale)` / `set_draw_distance_scale(scale)` this frame
    /// (#56); the host hands them to the renderer.
    pub lod_scale_request: Option<f32>,
    pub draw_distance_scale_request: Option<f32>,
    /// The renderer's scales now, for `lod_scale()` and
    /// `draw_distance_scale()`. The host sets them each frame.
    pub lod_scale: f32,
    pub draw_distance_scale: f32,
    /// The quality settings as scripts see them (FlintKart#55): what the
    /// host applied, with any `set_quality` this frame merged in.
    pub quality: flint_core::quality::QualitySettings,
    /// A `set_quality` call wants `quality` applied at the next frame.
    pub quality_requested: bool,
    /// What the GPU is, for `gpu_info()`. The host sets it once.
    pub gpu_info: flint_core::quality::GpuInfo,
    /// Whether the cursor is captured now, for `is_cursor_captured()`
    /// (ADR 0110). The host sets it each frame.
    pub cursor_captured: bool,
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
    /// Parsed TOML data files, keyed by resolved path (`load_data`)
    pub data_cache: std::collections::HashMap<String, toml::Value>,
    /// Terrain height sampling callback — set by PlayerApp if terrain is loaded
    pub terrain_height_fn: Option<Box<dyn Fn(f32, f32) -> f32 + Send + Sync>>,
    /// The game's ground, for `screen_to_heightfield` and `ground_height`
    /// (ADR 0095). Set through `Projector::set_ground`; survives `clear()`.
    pub ground: Option<Arc<flint_terrain::GroundField>>,
    /// The meshes drawn as ray targets, where the last frame drew them, for
    /// `raycast_meshes` and `screen_to_mesh` (ADR 0105). Set by the host
    /// after each render update (`ScriptSystem::set_ray_scene`).
    pub ray_scene: Arc<flint_core::ray_mesh::RayScene>,
    /// What `get_entity_style` and `get_style_param` ask (#372). Set by the
    /// host, which can see the renderer's style registry; survives `clear()`.
    pub style_reader: Option<Arc<dyn crate::style_reader::StyleReader>>,
    /// Real text measurement, installed by the host once a text backend
    /// exists: `(text, size, font family, letter spacing) -> (width, height)`
    /// in logical points. `measure_text` falls back to a glyph-count
    /// estimate when this is `None`.
    pub text_measurer: Option<TextMeasurer>,
    /// Persistent camera follow state (survives across frames)
    pub camera_follow: CameraFollowState,
    /// Persistent screen shake state
    pub shake: ShakeState,
    /// Set of currently loaded chunk IDs (synced from PlayerApp before scripts run)
    pub loaded_chunk_ids: HashSet<String>,
    /// Conducted-parameters snapshot for this frame (set by the host before
    /// scripts run; neutral defaults when no music session — ADR 0020)
    pub conducted: ConductedSnapshot,
    /// The latest expression state from outside the process (Flint#518):
    /// a timeline or a localhost socket, as the host feeds it. Empty until
    /// the first one arrives. `expression()` reads it; it outlives scene
    /// changes, since it describes the world outside the game, not the scene.
    pub expression: rhai::Map,
    /// The (script, function) pairs that have already warned about a setter
    /// given an entity that isn't there (#635), so each warns once.
    pub bad_target_warned: HashSet<(String, &'static str)>,
    /// Seconds into each playing `audio_source`'s sound, wrapped for a loop,
    /// for `audio_position` (#658). The host sets it each frame, paused or
    /// not; a source missing from it hasn't started.
    pub audio_positions: std::collections::HashMap<EntityId, f64>,
    /// How long after it is mixed a sound is heard, seconds, where the
    /// platform says (the browser), for `audio_output_latency`.
    pub audio_output_latency: Option<f64>,
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
            camera_view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            camera_direction: [0.0, 0.0, 1.0],
            current_entity: EntityId::from_raw(0),
            commands: Vec::new(),
            draw_commands: Vec::new(),
            world_lines: Vec::new(),
            world_texts: Vec::new(),
            clip_stack: Vec::new(),
            over_ui_depth: 0,
            mouse_pos_logical: (0.0, 0.0),
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
            time_scale_request: None,
            cursor_captured_override: None,
            touch_controls_override: None,
            lod_scale_request: None,
            draw_distance_scale_request: None,
            lod_scale: 1.0,
            draw_distance_scale: 1.0,
            quality: Default::default(),
            quality_requested: false,
            gpu_info: Default::default(),
            cursor_captured: false,
            state_machine: std::ptr::null_mut(),
            persistent_store: std::ptr::null_mut(),
            transition_progress: -1.0,
            transition_phase: String::from("idle"),
            current_scene_path: String::new(),
            ui_system: UiSystem::new(),
            data_cache: std::collections::HashMap::new(),
            terrain_height_fn: None,
            ground: None,
            ray_scene: Arc::default(),
            style_reader: None,
            text_measurer: None,
            camera_follow: CameraFollowState::default(),
            shake: ShakeState::default(),
            loaded_chunk_ids: HashSet::new(),
            conducted: ConductedSnapshot::default(),
            expression: rhai::Map::new(),
            bad_target_warned: HashSet::new(),
            audio_positions: std::collections::HashMap::new(),
            audio_output_latency: None,
        }
    }

    /// The entity a setter named in a script was given, or `None` when there
    /// is no such entity: `-1` from a `get_entity` that found nothing, or an
    /// id that was despawned (#635). The first time each script gives `func`
    /// such an id it warns, naming the script and the function, so a setter
    /// that would do nothing says so once instead of failing in silence.
    pub fn setter_target(&mut self, func: &'static str, id: i64) -> Option<EntityId> {
        if self.world.is_null() {
            return None;
        }
        // SAFETY: non-null, so set by the host for this call's scope
        let world = unsafe { &*self.world };
        let eid = EntityId::from_raw(id.max(0) as u64);
        if id >= 0 && world.contains(eid) {
            return Some(eid);
        }
        let caller = self.current_entity;
        let script = world
            .get_components(caller)
            .and_then(|c| c.get_field("script", "source"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| world.get_name(caller).map(|n| format!("entity '{n}'")))
            .unwrap_or_else(|| "a script".to_string());
        if self.bad_target_warned.insert((script.clone(), func)) {
            let why = if id < 0 {
                "(-1 is what get_entity returns for a name it can't find)"
            } else {
                "(no entity has that id; was it despawned?)"
            };
            tracing::warn!(
                "{script}: {func}({id}, ...) did nothing: there is no entity {id} {why}. \
                 Warned once for this script and function"
            );
        }
        None
    }

    /// Record a draw command, stamping the clip rect currently on the stack
    /// and whether it was issued over the UI.
    pub fn push_draw(&mut self, cmd: DrawCommand) {
        let clip = self.clip_stack.last().copied();
        let over_ui = self.over_ui_depth > 0;
        self.draw_commands.push(DrawItem { cmd, clip, over_ui });
    }

    /// Push a clip rect (intersected with the enclosing one, if any).
    pub fn push_clip(&mut self, rect: [f32; 4]) {
        let rect = match self.clip_stack.last() {
            Some(parent) => crate::draw2d::intersect_rect(*parent, rect),
            None => rect,
        };
        self.clip_stack.push(rect);
    }

    /// Pop the innermost clip rect; an unbalanced pop is ignored.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Start drawing over the game's egui. Pushes nest; each needs a pop.
    pub fn push_over_ui(&mut self) {
        self.over_ui_depth = self.over_ui_depth.saturating_add(1);
    }

    /// End the innermost `push_over_ui`; an unbalanced pop is ignored.
    pub fn pop_over_ui(&mut self) {
        self.over_ui_depth = self.over_ui_depth.saturating_sub(1);
    }

    /// Take this frame's draw items and reset the clip and over-UI stacks.
    pub fn take_draw_items(&mut self) -> Vec<DrawItem> {
        self.clip_stack.clear();
        self.over_ui_depth = 0;
        std::mem::take(&mut self.draw_commands)
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
    fn clip_stack_stamps_intersects_and_resets() {
        let mut c = ScriptCallContext::new();
        let line = || DrawCommand::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            color: [1.0; 4],
            thickness: 1.0,
            layer: 3,
        };
        c.push_draw(line());
        c.push_clip([0.0, 0.0, 100.0, 100.0]);
        c.push_draw(line());
        c.push_clip([50.0, 50.0, 100.0, 100.0]);
        c.push_draw(line());
        c.pop_clip();
        c.pop_clip();
        c.pop_clip(); // unbalanced: ignored
        c.push_draw(line());

        let items = c.take_draw_items();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].clip, None);
        assert_eq!(items[1].clip, Some([0.0, 0.0, 100.0, 100.0]));
        assert_eq!(items[2].clip, Some([50.0, 50.0, 50.0, 50.0]));
        assert_eq!(items[3].clip, None);
        assert_eq!(items[2].layer(), 3);

        // A frame that never pops still starts the next one clean.
        c.push_clip([1.0, 1.0, 1.0, 1.0]);
        let _ = c.take_draw_items();
        assert!(c.clip_stack.is_empty());
        assert!(c.draw_commands.is_empty());
    }

    #[test]
    fn over_ui_stamps_nests_and_resets() {
        let mut c = ScriptCallContext::new();
        let dot = || DrawCommand::CircleFilled {
            x: 0.0,
            y: 0.0,
            radius: 1.0,
            color: [1.0; 4],
            layer: 0,
        };
        c.push_draw(dot());
        c.push_over_ui();
        c.push_clip([0.0, 0.0, 10.0, 10.0]);
        c.push_draw(dot());
        c.pop_clip();
        c.push_over_ui();
        c.pop_over_ui();
        c.push_draw(dot()); // still inside the outer push
        c.pop_over_ui();
        c.pop_over_ui(); // unbalanced: ignored
        c.push_draw(dot());

        let items = c.take_draw_items();
        let over: Vec<bool> = items.iter().map(|i| i.over_ui).collect();
        assert_eq!(over, [false, true, true, false]);
        // The two stamps are independent.
        assert_eq!(items[1].clip, Some([0.0, 0.0, 10.0, 10.0]));
        assert_eq!(items[2].clip, None);

        // A frame that never pops still starts the next one under the UI.
        c.push_over_ui();
        let _ = c.take_draw_items();
        c.push_draw(dot());
        assert!(!c.take_draw_items()[0].over_ui);
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
