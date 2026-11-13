//! Script execution context and command types
//!
//! ScriptCallContext is the shared state accessed by Rhai API functions during script execution.
//! ScriptCommand represents deferred actions (audio, events, logging) collected during a script call,
//! which a [`crate::host::ScriptHost`] carries out.

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

/// Where a scene's music is this frame (#745): what `music_beat()`,
/// `music_bar()` and `music_section()` read. Set by the host's music
/// subsystem before scripts run; [`MusicClock::default`] (nothing playing)
/// with no music, so a script asks the same way in any scene.
#[derive(Clone, Debug, PartialEq)]
pub struct MusicClock {
    /// Whether a suite is playing (its clock is running).
    pub playing: bool,
    /// Beats from the suite's first beat, accumulated across tempo
    /// changes; negative in the count-in before it.
    pub beat: f64,
    /// The bar, counted from 0 at the suite's first beat.
    pub bar: i64,
    /// The section the music is in; "" before the first, or with none.
    pub section: String,
}

impl Default for MusicClock {
    fn default() -> Self {
        Self {
            playing: false,
            beat: -1.0,
            bar: -1,
            section: String::new(),
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
    /// A live entity: `emit_burst` resolves the id before queuing.
    EmitBurst {
        entity: EntityId,
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
    /// `ascii` (#269). The player adds the style's component. A live
    /// entity, as in every style command: the id was resolved before queuing.
    SetEntityStyle {
        entity: EntityId,
        style: String,
    },
    /// Change one setting of an entity's style: a parameter (number), an
    /// option (string), `enabled` (bool) or a 3-array such as `tint`.
    SetStyleParam {
        entity: EntityId,
        param: String,
        value: toml::Value,
    },
    /// Ease one setting of an entity's style (a number, `blend` or a
    /// colour) to `target` over `seconds` of game time (#370). The player
    /// writes the field each frame and fires `style_tween_done` at the end;
    /// a new tween of the same setting replaces the old one.
    TweenStyleParam {
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        /// `linear`, `smooth` (the default), `ease_in`, `ease_out` or
        /// `ease_in_out`
        easing: String,
    },
    /// Take an entity's own style off again.
    ClearEntityStyle {
        entity: EntityId,
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
    /// `tween_post_style_param` (#406): ease a whole-frame style's number
    /// (`reveal`, `blend`, a `reveal_*` key or a parameter) to `target` over
    /// `seconds` of game time. The engine writes it each frame and fires
    /// `post_style_tween_done` at the end; a new tween of the same setting
    /// replaces the old one, a `set_post_style_param` of it stops it.
    TweenPostStyleParam {
        style: String,
        param: String,
        target: f64,
        seconds: f64,
        /// `linear`, `smooth` (the default), `ease_in`, `ease_out` or
        /// `ease_in_out`
        easing: String,
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
    /// `set_panels(layout)` (#880): lay the frame out as comic panels, a
    /// `[panels]` table in the scene's form (gutter, border and
    /// `panel: #{ name: #{ rect, ... } }`), replacing the layout.
    SetPanels {
        layout: toml::Table,
    },
    /// `set_panel(name, keys)` (#880): merge `keys` into one panel of the
    /// layout, made if it isn't there (on the default page if there is none).
    SetPanel {
        name: String,
        keys: toml::Table,
    },
    /// `remove_panel(name)` (#880): take one panel off the page; the last
    /// one gone, the frame is one view again.
    RemovePanel {
        name: String,
    },
    /// `clear_panels()` (#880): the frame as one view again.
    ClearPanels,
    /// `rumble(pad, strength, seconds)` (#748): both motors of a pad (`pad`
    /// negative: every pad) at `strength` (0..1) for `seconds`; zero of
    /// either stops it. The host clamps both (`flint_runtime::RumbleRequest`).
    Rumble {
        pad: i64,
        strength: f64,
        seconds: f64,
    },
    /// Lay a look under the scene's `[post_process]` keys in place of the
    /// one it has (#356): a `styles/<name>.look.toml` or a built-in.
    /// `None` takes the look off (the scene's keys alone).
    SetLook {
        name: Option<String>,
    },
    /// A function of a module an optional feature owns (#698): Living
    /// Props' knobs, the wooze field, the A/B. The subsystem that declared
    /// the module takes it (`crate::module`); the core names none of them.
    Module(crate::module::ModuleCommand),
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
    /// A live entity: `set_velocity_2d` resolves the id before queuing.
    SetVelocity2D {
        entity: EntityId,
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
    // What a frame keeps one value of (#697): made from `FrameRequests` when
    // the host drains, after the commands above, one each for those asked.
    /// `set_camera_position`, `_target`, `_fov`, `_orthographic`,
    /// `_ortho_height`, `_roll` and `camera_follow`: the camera this frame.
    /// A field left `None` is the rig's; no roll is level.
    SetCamera {
        camera: CameraOverrides,
    },
    /// `set_vignette`, `set_bloom_intensity`, `set_exposure` and the other
    /// post-processing overrides: each held, once set, until set again.
    SetPostProcess {
        post: PostProcessOverrides,
    },
    /// `set_fog_color`, linear 0..1, held until set again.
    SetFogColor {
        color: [f32; 3],
    },
    /// `set_render_mode` (mode, mix: shown only the frames it is called)
    /// and `set_render_mode_params` (held).
    SetRenderMode {
        mode: Option<(u32, f32)>,
        params: Option<[f32; 4]>,
    },
    /// `set_time_scale`, clamped to 0.05..4, from the next frame.
    SetTimeScale {
        scale: f64,
    },
    /// `set_audio_lowpass`: the master bus low-pass cutoff.
    SetAudioLowpass {
        cutoff_hz: f32,
    },
    /// `set_touch_controls` (#134).
    SetTouchControls {
        enabled: bool,
    },
    /// `set_lod_scale` / `set_draw_distance_scale` (#56).
    SetLodScales {
        lod_scale: Option<f32>,
        draw_distance_scale: Option<f32>,
    },
    /// `set_cursor_captured`.
    SetCursorCaptured {
        captured: bool,
    },
    /// `set_quality`: the whole settings, the call's keys merged in
    /// (FlintKart#55).
    SetQuality {
        settings: flint_core::quality::QualitySettings,
    },
}

/// The post-processing overrides scripts set: each `Some` only if its
/// `set_*` was called since the last drain.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PostProcessOverrides {
    pub vignette: Option<f32>,
    pub bloom: Option<f32>,
    pub exposure: Option<f32>,
    pub chromatic_aberration: Option<f32>,
    pub radial_blur: Option<f32>,
    pub ssao_intensity: Option<f32>,
    pub fog_density: Option<f32>,
    pub desaturation: Option<f32>,
    pub dof_strength: Option<f32>,
    pub dof_focus_distance: Option<f32>,
    pub dof_focus_range: Option<f32>,
}

impl PostProcessOverrides {
    /// Lay `later` over these: what it sets wins, what it leaves stays.
    pub fn merge(&mut self, later: PostProcessOverrides) {
        macro_rules! keep {
            ($($field:ident),*) => {
                $(if later.$field.is_some() { self.$field = later.$field; })*
            };
        }
        keep!(
            vignette,
            bloom,
            exposure,
            chromatic_aberration,
            radial_blur,
            ssao_intensity,
            fog_density,
            desaturation,
            dof_strength,
            dof_focus_distance,
            dof_focus_range
        );
    }
}

/// The camera overrides scripts set: each `Some` only if its
/// `set_camera_*` was called since the last drain.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CameraOverrides {
    pub position: Option<[f32; 3]>,
    pub target: Option<[f32; 3]>,
    pub fov: Option<f32>,
    pub orthographic: Option<bool>,
    pub ortho_height: Option<f32>,
    /// Roll about the view axis, radians. `None` means "no roll this frame" —
    /// the host resets the camera up vector to world up.
    pub roll: Option<f32>,
}

impl CameraOverrides {
    /// Lay `later` over these: what it sets wins, what it leaves stays.
    pub fn merge(&mut self, later: CameraOverrides) {
        self.position = later.position.or(self.position);
        self.target = later.target.or(self.target);
        self.fov = later.fov.or(self.fov);
        self.orthographic = later.orthographic.or(self.orthographic);
        self.ortho_height = later.ortho_height.or(self.ortho_height);
        self.roll = later.roll.or(self.roll);
    }
}

/// What scripts asked of this frame that the frame keeps one value of: the
/// last call wins, and some are read back before the host has them
/// (`apply_camera_shake` shakes the camera asked for, `get_lod_scale()` says
/// the scale asked for). [`ScriptCallContext::take_commands`] turns them
/// into commands.
#[derive(Debug, Clone, Default)]
pub struct FrameRequests {
    pub camera: CameraOverrides,
    pub post: PostProcessOverrides,
    pub fog_color: Option<[f32; 3]>,
    /// Shown only the frames a script asks for it (a reality tear).
    pub render_mode: Option<(u32, f32)>,
    pub mode_params: Option<[f32; 4]>,
    pub time_scale: Option<f64>,
    pub audio_lowpass: Option<f32>,
    pub cursor_captured: Option<bool>,
    pub touch_controls: Option<bool>,
    pub lod_scale: Option<f32>,
    pub draw_distance_scale: Option<f32>,
    /// A `set_quality` call changed `ScriptCallContext::quality`.
    pub quality: bool,
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
    /// What scripts asked of the frame since the host last drained: the
    /// camera, post-processing and the other one-value-a-frame settings
    /// (#697). The host gets them as commands, from the same drain as the
    /// rest ([`Self::take_commands`]).
    pub requests: FrameRequests,
    /// The renderer's scales now, for `get_lod_scale()` and
    /// `get_draw_distance_scale()`. The host sets them each frame.
    pub lod_scale: f32,
    pub draw_distance_scale: f32,
    /// The quality settings as scripts see them (FlintKart#55): what the
    /// host applied, with any `set_quality` this frame merged in.
    pub quality: flint_core::quality::QualitySettings,
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
    /// The project the current scene belongs to (ADR 0114), handed in with
    /// the scene path (`ScriptSystem::set_current_scene`). `None` until a
    /// host says: then it is found from the scene path when asked for.
    pub project: Option<flint_core::project::Project>,
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
    pub ray_scene: Arc<flint_geometry::ray_mesh::RayScene>,
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
    /// The modules an optional feature owns that may run now (#698): what
    /// the host's subsystems for the scene are (`living`, `conducted`...).
    /// A gated module's function checks it before it runs.
    pub enabled_modules: Vec<&'static str>,
    /// What the host hands those modules' functions (a rhythm session's
    /// snapshot), one value of each type.
    pub module_state: crate::module::ModuleState,
    /// Where the scene's music is this frame (#745); nothing playing when
    /// the scene has none.
    pub music: MusicClock,
    /// The latest expression state from outside the process (Flint#518):
    /// a timeline or a localhost socket, as the host feeds it. Empty until
    /// the first one arrives. `expression()` reads it; it outlives scene
    /// changes, since it describes the world outside the game, not the scene.
    pub expression: rhai::Map,
    /// The (script, function) pairs that have already warned about a setter
    /// given an entity that isn't there (#635), so each warns once.
    pub bad_target_warned: HashSet<(String, &'static str)>,
    /// The (function, key) pairs whose opts map has already warned about a
    /// key it doesn't take or a value of the wrong shape (#702).
    pub opts_warned: HashSet<(&'static str, String)>,
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
    /// The directory a script's path "from the project root" (`load_ui`,
    /// `load_data`) is read under: the current scene's project's root
    /// (`Project::root_for`, ADR 0114). TEMPORARY, for one release: when
    /// `rel` is only where the root used to be guessed, that place, with a
    /// warning (`Project::root_holding`). With no scene yet, the working
    /// directory.
    pub fn project_root_holding(&self, what: &str, rel: &str) -> std::path::PathBuf {
        use std::path::{Path, PathBuf};
        if self.current_scene_path.is_empty() {
            return PathBuf::from(".");
        }
        let scene = Path::new(&self.current_scene_path);
        let scene_dir = scene
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        match &self.project {
            Some(project) => project.root_holding(what, rel, scene_dir),
            None => {
                flint_core::project::Project::for_scene(scene).root_holding(what, rel, scene_dir)
            }
        }
    }

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
            requests: FrameRequests::default(),
            lod_scale: 1.0,
            draw_distance_scale: 1.0,
            quality: Default::default(),
            gpu_info: Default::default(),
            cursor_captured: false,
            state_machine: std::ptr::null_mut(),
            persistent_store: std::ptr::null_mut(),
            transition_progress: -1.0,
            transition_phase: String::from("idle"),
            current_scene_path: String::new(),
            project: None,
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
            enabled_modules: Vec::new(),
            module_state: Default::default(),
            music: MusicClock::default(),
            expression: rhai::Map::new(),
            bad_target_warned: HashSet::new(),
            opts_warned: HashSet::new(),
            audio_positions: std::collections::HashMap::new(),
            audio_output_latency: None,
        }
    }

    /// Whether the host has enabled `module`, one an optional feature owns
    /// (#698).
    pub fn module_enabled(&self, module: &str) -> bool {
        self.enabled_modules.contains(&module)
    }

    /// The entity a setter named in a script was given, or `None` when there
    /// is no such entity: `-1` from a `get_entity` that found nothing, or an
    /// id that was despawned (#635). The first time each script gives `func`
    /// such an id it warns, naming the script and the function, so a setter
    /// that would do nothing says so once instead of failing in silence.
    pub fn setter_target(&mut self, func: &'static str, id: i64) -> Option<EntityId> {
        self.entity_target(func, id, None)
    }

    /// [`Self::setter_target`] for every function the declared API builds
    /// (ADR 0116): an `Entity` argument resolves through here, `line` being
    /// where the script called it, when Rhai says.
    pub fn entity_target(
        &mut self,
        func: &'static str,
        id: i64,
        line: Option<usize>,
    ) -> Option<EntityId> {
        let world = self.try_world()?;
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
            let at = line.map(|l| format!(" line {l}:")).unwrap_or_default();
            tracing::warn!(
                "{script}:{at} {func}({id}, ...) did nothing: there is no entity {id} {why}. \
                 Warned once for this script and function"
            );
        }
        None
    }

    /// The world scripts are running against, or `None` outside a call
    /// scope (a script's top level runs once at load with no world).
    ///
    /// The one safe way to it (ADR 0116): the host sets `world` for the
    /// scope of a call batch (`WorldScope`) and nulls it after, and the
    /// borrow handed out here is tied to this context, which the registered
    /// function holds locked for the length of its call.
    pub fn try_world(&self) -> Option<&FlintWorld> {
        // SAFETY: non-null only inside a host's call scope, where it points
        // at the world the host lent for that scope and is not touching
        (!self.world.is_null()).then(|| unsafe { &*self.world })
    }

    /// [`Self::try_world`], mutably. Taking `&mut self` means a function
    /// holds at most one `&mut FlintWorld` at a time.
    pub fn try_world_mut(&mut self) -> Option<&mut FlintWorld> {
        // SAFETY: as `try_world`; the `&mut self` borrow keeps it unique
        (!self.world.is_null()).then(|| unsafe { &mut *self.world })
    }

    /// The physics system, when the host has lent one for this scope.
    pub fn try_physics(&self) -> Option<&PhysicsSystem> {
        // SAFETY: as `try_world`
        (!self.physics.is_null()).then(|| unsafe { &*self.physics })
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

    /// Take what scripts asked of the host since the last call: the queued
    /// commands in the order they were made, then this frame's requests
    /// (camera, post-processing, time scale...), one command each, for
    /// those asked for. A distance scale asked for reads back from here on
    /// (`get_lod_scale()`) until the host says what it applied.
    pub fn take_commands(&mut self) -> Vec<ScriptCommand> {
        let mut out = std::mem::take(&mut self.commands);
        let r = std::mem::take(&mut self.requests);
        if r.camera != CameraOverrides::default() {
            out.push(ScriptCommand::SetCamera { camera: r.camera });
        }
        if r.post != PostProcessOverrides::default() {
            out.push(ScriptCommand::SetPostProcess { post: r.post });
        }
        if let Some(color) = r.fog_color {
            out.push(ScriptCommand::SetFogColor { color });
        }
        if r.render_mode.is_some() || r.mode_params.is_some() {
            out.push(ScriptCommand::SetRenderMode {
                mode: r.render_mode,
                params: r.mode_params,
            });
        }
        if let Some(scale) = r.time_scale {
            out.push(ScriptCommand::SetTimeScale { scale });
        }
        if let Some(cutoff_hz) = r.audio_lowpass {
            out.push(ScriptCommand::SetAudioLowpass { cutoff_hz });
        }
        if let Some(enabled) = r.touch_controls {
            out.push(ScriptCommand::SetTouchControls { enabled });
        }
        if r.lod_scale.is_some() || r.draw_distance_scale.is_some() {
            self.lod_scale = r.lod_scale.unwrap_or(self.lod_scale);
            self.draw_distance_scale = r.draw_distance_scale.unwrap_or(self.draw_distance_scale);
            out.push(ScriptCommand::SetLodScales {
                lod_scale: r.lod_scale,
                draw_distance_scale: r.draw_distance_scale,
            });
        }
        if let Some(captured) = r.cursor_captured {
            out.push(ScriptCommand::SetCursorCaptured { captured });
        }
        if r.quality {
            out.push(ScriptCommand::SetQuality {
                settings: self.quality,
            });
        }
        out
    }

    /// The game state machine, when the host has lent one for this scope
    /// (`StateScope`).
    pub fn try_state_machine(&self) -> Option<&GameStateMachine> {
        // SAFETY: as `try_world`: set by `StateScope` for its scope, nulled
        // on drop
        (!self.state_machine.is_null()).then(|| unsafe { &*self.state_machine })
    }

    /// [`Self::try_state_machine`], mutably.
    pub fn try_state_machine_mut(&mut self) -> Option<&mut GameStateMachine> {
        // SAFETY: as `try_world_mut`
        (!self.state_machine.is_null()).then(|| unsafe { &mut *self.state_machine })
    }

    /// The persistent store, when the host has lent one for this scope
    /// (`StateScope`).
    pub fn try_persistent_store(&self) -> Option<&PersistentStore> {
        // SAFETY: as `try_state_machine`
        (!self.persistent_store.is_null()).then(|| unsafe { &*self.persistent_store })
    }

    /// [`Self::try_persistent_store`], mutably.
    pub fn try_persistent_store_mut(&mut self) -> Option<&mut PersistentStore> {
        // SAFETY: as `try_world_mut`
        (!self.persistent_store.is_null()).then(|| unsafe { &mut *self.persistent_store })
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
