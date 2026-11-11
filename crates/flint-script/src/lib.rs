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
pub mod draw2d;
pub mod engine;
pub mod module_resolver;
pub mod projector;
pub mod random;
#[cfg(not(target_arch = "wasm32"))]
mod reload_worker;
pub mod style_reader;
pub mod sync;
pub mod ui;

pub use context::StateScope;
pub use context::{ConductedCue, ConductedPulse, ConductedSnapshot, CueParam};
pub use context::{DrawCommand, DrawItem};
use context::{InputSnapshot, ScriptCommand};
use engine::ScriptEngine;
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::{GameEvent, InputState, RuntimeSystem};
pub use projector::{GroundField, Projector, ScreenPoint, View};
pub use style_reader::{EntityStyle, StyleReader};
use sync::ScriptSync;

/// Post-processing overrides set by scripts this frame; each field is `Some`
/// only if the corresponding `set_*` API was called since the last drain.
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

/// Camera overrides set by scripts this frame; each field is `Some` only if
/// the corresponding `set_camera_*` API was called since the last drain.
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

/// Top-level scripting system integrating engine, sync, and the game loop
pub struct ScriptSystem {
    pub(crate) engine: ScriptEngine,
    pub(crate) sync: ScriptSync,
    pending_events: Vec<GameEvent>,
    /// Expression states from outside the process, waiting for the next
    /// `update` (Flint#518). Not cleared by a scene change: they describe the
    /// world outside the game.
    pending_expressions: std::collections::VecDeque<rhai::Map>,
}

/// The most expression states held for scripts that aren't running (a paused
/// state, a long load). Past this the oldest go first: each state is a whole
/// snapshot, so the latest is the one that matters.
pub const MAX_PENDING_EXPRESSIONS: usize = 256;

impl ScriptSystem {
    /// Time each script's `on_update` from now on, for a perf readout.
    pub fn set_update_timing(&mut self, on: bool) {
        self.engine.set_update_timing(on);
    }

    /// Each script file's time in the last frame's `on_update`, ms; empty
    /// unless [`set_update_timing`](Self::set_update_timing) is on.
    pub fn update_times(&self) -> &[(String, f32)] {
        self.engine.update_times()
    }

    pub fn new() -> Self {
        Self {
            engine: ScriptEngine::new(),
            sync: ScriptSync::new(),
            pending_events: Vec::new(),
            pending_expressions: std::collections::VecDeque::new(),
        }
    }

    /// Queue an expression state from outside the process (Flint#518). The
    /// next `update` makes it what `expression()` returns and calls
    /// `on_expression(state)` on every script that defines it, before any
    /// `on_update`, so a state queued this frame is seen this frame.
    pub fn queue_expression(&mut self, state: rhai::Map) {
        if self.pending_expressions.len() >= MAX_PENDING_EXPRESSIONS {
            self.pending_expressions.pop_front();
            tracing::warn!(
                "expression: more than {MAX_PENDING_EXPRESSIONS} states waiting for scripts; dropped the oldest"
            );
        }
        self.pending_expressions.push_back(state);
    }

    /// Make the game clock the only clock scripts have, and their randomness a
    /// seeded sequence, for runs that must come out the same every time
    /// (`flint render --scripts`, #8). `random()` and `random_range(a, b)` draw
    /// from one SplitMix64 started at `seed`, shared by every script in the
    /// order they run; Rhai's `timestamp()`, which reads the wall clock, fails
    /// and says to use `total_time()`. The player keeps the clock, and
    /// seeds only `random()` ([`Self::seed_random`]).
    pub fn make_deterministic(&mut self, seed: u64) {
        self.seed_random(seed);
        self.rhai_engine_mut().register_fn(
            "timestamp",
            || -> std::result::Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
                Err(
                    "timestamp() reads the wall clock, which a deterministic run doesn't \
                     have: use total_time() or delta_time()"
                        .into(),
                )
            },
        );
    }

    /// Restart `random()` and `random_range(a, b)` at `seed`: one
    /// SplitMix64 sequence shared by every script in the order they call it.
    /// Without this it starts from the wall clock, once, when the engine is
    /// made (#580). The player's `--seed` (and `--frame-step`, #635), `flint
    /// render --scripts` and a script's own `seed_random(n)` all come here,
    /// so the same seed gives the same numbers. The sequence lives as long as
    /// the engine, through scene changes.
    pub fn seed_random(&mut self, seed: u64) {
        self.engine.random.seed(seed);
    }

    /// The expression state scripts see now (`expression()`): the last one
    /// delivered, or an empty map before any.
    pub fn expression(&self) -> rhai::Map {
        crate::lock_or_recover(&self.engine.ctx).expression.clone()
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
            any_just_pressed: input.any_just_pressed(),
            last_device: input.last_device().as_str().to_string(),
            gamepad_connected: input.gamepad_connected(),
            touch_controls_visible: input.touch_controls_visible(),
            touch_device: input.touch_device(),
            safe_area: input.touch_controls().insets().map(f64::from),
            mouse_delta: input.raw_mouse_delta(),
            mouse_wheel: {
                let (x, y) = input.mouse_wheel_delta();
                (x as f64, y as f64)
            },
            mouse_buttons_down: (0..3_u32)
                .filter(|b| input.is_mouse_button_down(*b))
                .map(|b| b as i64)
                .collect(),
            mouse_buttons_just_pressed: (0..3_u32)
                .filter(|b| input.is_mouse_button_just_pressed(*b))
                .map(|b| b as i64)
                .collect(),
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
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.transition_progress = progress;
        c.transition_phase = phase.to_string();
    }

    /// Set the current scene's path and the project it belongs to, for
    /// script access: `load_ui` and `load_data` paths are from the
    /// project's root (ADR 0114).
    pub fn set_current_scene(&mut self, path: &str, project: &flint_core::project::Project) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        // Called every frame: nothing is copied while the scene stays
        if c.current_scene_path != path {
            c.current_scene_path = path.to_string();
        }
        if c.project.as_ref() != Some(project) {
            c.project = Some(project.clone());
        }
    }

    /// Set camera position and direction for weapon aiming
    pub fn set_camera(&mut self, position: [f32; 3], direction: [f32; 3]) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.camera_position = position;
        c.camera_direction = direction;
    }

    /// Camera view-projection (column-major) for world-space debug drawing
    /// (`draw_line_3d`, `world_to_screen`). Pass the matrix the frame renders with.
    pub fn set_camera_view_proj(&mut self, view_proj: [[f32; 4]; 4]) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.camera_view_proj = view_proj;
    }

    /// A cloneable handle on the camera `world_to_screen` uses, for native
    /// functions a game registers (ADR 0087). Inside a script call it's
    /// the camera this frame renders with; see [`Projector`].
    pub fn projector(&self) -> Projector {
        Projector::new(self.engine.ctx.clone())
    }

    /// The meshes drawn as ray targets (ADR 0105), which `raycast_meshes`,
    /// `screen_to_mesh` and [`Projector::ray_scene`] cast against. The host
    /// hands over the renderer's after each update, so scripts see where the
    /// last frame drew them.
    pub fn set_ray_scene(&mut self, scene: std::sync::Arc<flint_core::ray_mesh::RayScene>) {
        crate::lock_or_recover(&self.engine.ctx).ray_scene = scene;
    }

    /// What scripts' `get_entity_style` and `get_style_param` ask (#372);
    /// see [`StyleReader`].
    pub fn set_style_reader(&mut self, reader: std::sync::Arc<dyn StyleReader>) {
        self.engine.set_style_reader(reader);
    }

    /// Drain commands produced by scripts this frame
    pub fn drain_commands(&mut self) -> Vec<ScriptCommand> {
        self.engine.drain_commands()
    }

    /// Take the quality settings a script asked for with `set_quality`
    /// since the last call, if any (FlintKart#55). The host applies them
    /// between frames and hands back what it applied with
    /// [`set_quality_state`](Self::set_quality_state).
    pub fn take_quality_request(&mut self) -> Option<flint_core::quality::QualitySettings> {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        std::mem::take(&mut c.quality_requested).then_some(c.quality)
    }

    /// The quality settings in force, for `get_quality()`.
    pub fn set_quality_state(&mut self, quality: flint_core::quality::QualitySettings) {
        crate::lock_or_recover(&self.engine.ctx).quality = quality;
    }

    /// What the GPU is, for `gpu_info()`.
    pub fn set_gpu_info(&mut self, info: flint_core::quality::GpuInfo) {
        crate::lock_or_recover(&self.engine.ctx).gpu_info = info;
    }

    /// Set screen dimensions for UI draw functions
    pub fn set_screen_size(&mut self, w: f32, h: f32) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.screen_width = w;
        c.screen_height = h;
    }

    /// Set the cursor position in logical points (the `screen_width` space)
    /// for `mouse_x()` / `mouse_y()`. The host divides the physical cursor
    /// position by its pixels-per-point before calling this.
    pub fn set_mouse_position(&mut self, x: f32, y: f32) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.mouse_pos_logical = (x, y);
    }

    /// Whether the cursor is captured now, for `is_cursor_captured()`
    /// (ADR 0110).
    pub fn set_cursor_captured(&mut self, captured: bool) {
        crate::lock_or_recover(&self.engine.ctx).cursor_captured = captured;
    }

    /// Whether `is_cursor_captured()` reads true (ADR 0110).
    pub fn cursor_captured(&self) -> bool {
        crate::lock_or_recover(&self.engine.ctx).cursor_captured
    }

    /// Sync loaded chunk IDs so scripts can query `is_chunk_loaded`
    pub fn set_loaded_chunk_ids(&mut self, ids: std::collections::HashSet<String>) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.loaded_chunk_ids = ids;
    }

    /// Set the conducted-parameters snapshot for this frame (ADR 0020).
    /// Persistent, not take(): the host re-sets it every frame, passing
    /// `ConductedSnapshot::default()` (the neutral, settled-world state)
    /// when no music session is active.
    pub fn set_conducted(&mut self, conducted: ConductedSnapshot) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.conducted = conducted;
    }

    /// Where each playing `audio_source` is in its sound, and the output
    /// latency where known, for `audio_position` / `audio_output_latency`
    /// (#658). The host sets them every frame before scripts run, paused or
    /// not.
    pub fn set_audio_clock(
        &mut self,
        positions: &std::collections::HashMap<flint_core::EntityId, f64>,
        output_latency: Option<f64>,
    ) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.audio_positions.clone_from(positions);
        c.audio_output_latency = output_latency;
    }

    /// Load the script a `script.source` names for a specific entity (for
    /// chunk loading), searched for from `from_dir`, the chunk's own
    /// directory, by the project's one rule (`AssetKind::Scripts`), and
    /// followed by hot reload. The entity must already have a `script`
    /// component in the ECS. `Err` is the not-found text, every path tried;
    /// nothing is loaded then.
    pub fn load_script_named(
        &mut self,
        entity_id: flint_core::EntityId,
        source: &str,
        from_dir: &std::path::Path,
    ) -> std::result::Result<std::path::PathBuf, String> {
        let script_path = self.sync.locate_for(entity_id, source, from_dir)?;
        self.load_script_for_entity(entity_id, &script_path);
        Ok(script_path)
    }

    /// Load a script file for a specific entity, from the path given.
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
                tracing::warn!("Compile error for chunk entity {:?}: {}", entity_id, e);
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

    /// Drain draw commands produced by scripts this frame (each stamped
    /// with the clip rect active when it was issued)
    pub fn drain_draw_commands(&mut self) -> Vec<DrawItem> {
        self.engine.drain_draw_commands()
    }

    /// Drain the depth-tested world lines (`draw_world_line`, ADR 0083)
    /// scripts queued this frame, for `SceneRenderer::set_world_lines`
    pub fn drain_world_lines(&mut self) -> Vec<flint_core::world_line::WorldLine> {
        self.engine.drain_world_lines()
    }

    /// Drain the depth-tested world text (`draw_world_text`, ADR 0104)
    /// scripts queued this frame, for the host to lay out and draw
    pub fn drain_world_texts(&mut self) -> Vec<flint_core::world_text::WorldText> {
        self.engine.drain_world_texts()
    }

    /// Generate draw commands from the data-driven UI system (unclipped)
    pub fn generate_ui_draw_commands(&mut self, screen_w: f32, screen_h: f32) -> Vec<DrawItem> {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.ui_system
            .generate_draw_commands(screen_w, screen_h)
            .into_iter()
            .map(DrawItem::unclipped)
            .collect()
    }

    /// Call on_animation_end(clip_name) for entities whose sprite animation finished
    pub fn call_sprite_anim_ends(
        &mut self,
        world: &mut FlintWorld,
        events: &[flint_animation::sprite_sync::SpriteAnimEndEvent],
    ) {
        self.engine.call_sprite_anim_ends(world, events);
    }

    /// Call on_sequence_cue(sequence, cue) for cues animation sequences passed
    pub fn call_sequence_cues(
        &mut self,
        world: &mut FlintWorld,
        cues: &[flint_animation::SequenceCueEvent],
    ) {
        self.engine.call_sequence_cues(world, cues);
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
    pub fn take_camera_overrides(&mut self) -> CameraOverrides {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        CameraOverrides {
            position: c.camera_position_override.take(),
            target: c.camera_target_override.take(),
            fov: c.camera_fov_override.take(),
            orthographic: c.camera_orthographic_override.take(),
            ortho_height: c.camera_ortho_height_override.take(),
            roll: c.camera_roll_override.take(),
        }
    }

    /// Take post-processing overrides set by scripts this frame (clears them)
    pub fn take_postprocess_overrides(&mut self) -> PostProcessOverrides {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        PostProcessOverrides {
            vignette: c.postprocess_vignette_override.take(),
            bloom: c.postprocess_bloom_override.take(),
            exposure: c.postprocess_exposure_override.take(),
            chromatic_aberration: c.postprocess_chromatic_aberration_override.take(),
            radial_blur: c.postprocess_radial_blur_override.take(),
            ssao_intensity: c.postprocess_ssao_intensity_override.take(),
            fog_density: c.postprocess_fog_density_override.take(),
            desaturation: c.postprocess_desaturation_override.take(),
            dof_strength: c.postprocess_dof_strength_override.take(),
            dof_focus_distance: c.postprocess_dof_focus_distance_override.take(),
            dof_focus_range: c.postprocess_dof_focus_range_override.take(),
        }
    }

    /// Take the time scale a script asked for this frame, if any (clears it)
    pub fn take_time_scale_request(&mut self) -> Option<f64> {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.time_scale_request.take()
    }

    /// Take audio low-pass filter override set by scripts this frame (clears it)
    pub fn take_audio_overrides(&mut self) -> Option<f32> {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.audio_lowpass_cutoff_override.take()
    }

    /// Take the `set_touch_controls` request set by scripts this frame
    /// (clears it)
    pub fn take_touch_controls_override(&mut self) -> Option<bool> {
        crate::lock_or_recover(&self.engine.ctx)
            .touch_controls_override
            .take()
    }

    /// Take the `set_lod_scale` / `set_draw_distance_scale` requests made by
    /// scripts this frame (clears them), as (lod scale, draw-distance scale).
    pub fn take_lod_requests(&mut self) -> (Option<f32>, Option<f32>) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        (
            c.lod_scale_request.take(),
            c.draw_distance_scale_request.take(),
        )
    }

    /// Tell scripts the renderer's distance scales in force (#56), for
    /// `lod_scale()` and `draw_distance_scale()`.
    pub fn set_lod_state(&mut self, lod_scale: f32, draw_distance_scale: f32) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.lod_scale = lod_scale;
        c.draw_distance_scale = draw_distance_scale;
    }

    /// Take the cursor capture request set by scripts this frame (clears it)
    pub fn take_cursor_capture_override(&mut self) -> Option<bool> {
        let mut c = self.engine.ctx.lock().unwrap();
        c.cursor_captured_override.take()
    }

    /// Take the fog color override set by scripts this frame (clears it)
    pub fn take_fog_color_override(&mut self) -> Option<[f32; 3]> {
        let mut c = self.engine.ctx.lock().unwrap();
        c.postprocess_fog_color_override.take()
    }

    /// Take the reality-tear render mode overrides set by scripts this
    /// frame (clears them): ((mode, mix), mode_params).
    #[allow(clippy::type_complexity)]
    pub fn take_render_mode_overrides(&mut self) -> (Option<(u32, f32)>, Option<[f32; 4]>) {
        let mut c = self.engine.ctx.lock().unwrap();
        (
            c.postprocess_render_mode_override.take(),
            c.postprocess_mode_params_override.take(),
        )
    }

    /// Point the script system at the scripts the scene at `scene_path` can
    /// see in `project` (`AssetKind::Scripts`: `scripts/` beside the scene,
    /// in each directory above it up to the project root, then in the
    /// engine). Also points `import` at `scripts/lib/` in the same places
    /// and starts watching them.
    pub fn load_scripts_from_scene(
        &mut self,
        scene_path: &str,
        project: &flint_core::project::Project,
    ) {
        sync::load_scripts_from_scene(scene_path, project, &mut self.sync);
        if let Some(base) = self.sync.module_base() {
            self.engine.set_module_search(base);
        }
    }

    /// The Rhai engine, for a host that registers its own native functions
    /// (flint-player's `PlayerExtension::register_script_api`). Flint's API
    /// is already registered; a later registration with the same name and
    /// parameter types replaces it. The engine survives `clear()`, so
    /// registrations last across scene transitions. With rhai's `sync`
    /// feature, registered closures must be `Send + Sync`.
    pub fn rhai_engine_mut(&mut self) -> &mut rhai::Engine {
        self.engine.rhai_engine_mut()
    }

    /// Set the terrain height sampling callback for scripts
    pub fn set_terrain_height_fn(&mut self, f: Option<Box<dyn Fn(f32, f32) -> f32 + Send + Sync>>) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.terrain_height_fn = f;
    }

    /// Install (or clear) the host's real text measurer used by
    /// `measure_text` / `measure_text_ex`.
    pub fn set_text_measurer(&mut self, f: Option<context::TextMeasurer>) {
        let mut c = crate::lock_or_recover(&self.engine.ctx);
        c.text_measurer = f;
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
            eprintln!("Script system initialized ({} scripts)", count);
        }

        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Scripts run at variable rate (like animation), not fixed timestep
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // A perf readout times each step below, and each script's on_update.
        self.engine.clear_update_times();
        let timing = self.engine.timing();
        let now = || timing.then(web_time::Instant::now);

        // Check for hot-reloaded scripts
        let began = now();
        self.sync.check_hot_reload(&mut self.engine);

        // Check for edited .ui.toml / .style.toml / token files
        crate::lock_or_recover(&self.engine.ctx)
            .ui_system
            .poll_reload();
        if let Some(began) = began {
            self.engine.add_update_time("[hot reload]", began);
        }

        // Discover any new script entities
        let began = now();
        self.sync.discover_and_load(world, &mut self.engine);

        // Call on_init for newly added scripts
        self.engine.call_inits(world);
        if let Some(began) = began {
            self.engine.add_update_time("[load, on_init]", began);
        }

        // Process events (collision, trigger, action callbacks)
        let began = now();
        let events = std::mem::take(&mut self.pending_events);
        self.engine.process_events(&events, world);
        if let Some(began) = began {
            self.engine.add_update_time("[events]", began);
        }

        // Expression states from outside (Flint#518), so on_update sees them
        let began = now();
        let states: Vec<rhai::Map> = self.pending_expressions.drain(..).collect();
        self.engine.deliver_expressions(world, states);
        if let Some(began) = began {
            self.engine.add_update_time("[expression]", began);
        }

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

    #[test]
    fn host_registered_functions_reach_scripts_and_survive_clear() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::<i64>::new()));
        let mut system = ScriptSystem::new();
        {
            let seen = seen.clone();
            let rhai = system.rhai_engine_mut();
            rhai.register_fn("host_answer", || 42_i64);
            rhai.register_fn("host_record", move |v: i64| seen.lock().unwrap().push(v));
            // Replaces Flint's own delta_time() for every script.
            rhai.register_fn("delta_time", || 7.0_f64);
        }

        let source =
            "fn on_update() { host_record(host_answer()); host_record(delta_time().to_int()); }";
        let mut world = FlintWorld::new();
        let id = world.spawn("host").unwrap();
        let ast = system.engine.compile(source).unwrap();
        system.engine.add_script(id, ast, "host.rhai".into());
        system.engine.call_updates(&mut world);
        assert_eq!(*seen.lock().unwrap(), [42, 7]);

        // A scene transition keeps the engine and its registrations.
        system.clear();
        let id = world.spawn("host2").unwrap();
        let ast = system.engine.compile(source).unwrap();
        system.engine.add_script(id, ast, "host.rhai".into());
        system.engine.call_updates(&mut world);
        assert_eq!(*seen.lock().unwrap(), [42, 7, 42, 7]);
    }

    #[test]
    fn expression_states_reach_scripts_in_order_before_on_update() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut system = ScriptSystem::new();
        {
            let seen = seen.clone();
            system
                .rhai_engine_mut()
                .register_fn("record", move |s: &str| seen.lock().unwrap().push(s.into()));
        }
        let source = r#"
            fn on_expression(state) {
                // expression() already says what this call is about
                record(`heard ${state.state} (now ${expression().state}) energy ${state.energy}`);
            }
            fn on_update() {
                let e = expression();
                record(if e.len() == 0 { "update: none" } else { `update: ${e.state}` });
            }
        "#;
        let mut world = FlintWorld::new();
        let id = world.spawn("listener").unwrap();
        let ast = system.engine.compile(source).unwrap();
        system.engine.add_script(id, ast, "listener.rhai".into());

        system.update(&mut world, 0.016).unwrap();
        let state = |name: &str, energy: f64| {
            let mut m = rhai::Map::new();
            m.insert("state".into(), name.into());
            m.insert("energy".into(), rhai::Dynamic::from_float(energy));
            m
        };
        system.queue_expression(state("thinking", 0.6));
        system.queue_expression(state("speaking", 0.5));
        system.update(&mut world, 0.016).unwrap();
        system.update(&mut world, 0.016).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "update: none",
                "heard thinking (now thinking) energy 0.6",
                "heard speaking (now speaking) energy 0.5",
                "update: speaking",
                "update: speaking", // it holds until another arrives
            ]
        );

        // A scene change keeps the state: it's the world outside the game.
        system.clear();
        assert_eq!(
            system.expression()["state"].clone().into_string().unwrap(),
            "speaking"
        );
    }

    /// A system whose scripts can `record(s)` what they see, and what they saw.
    fn recording_system() -> (ScriptSystem, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut system = ScriptSystem::new();
        let into = seen.clone();
        system
            .rhai_engine_mut()
            .register_fn("record", move |s: &str| into.lock().unwrap().push(s.into()));
        (system, seen)
    }

    #[test]
    fn deterministic_random_is_the_same_sequence_every_run() {
        let source = r#"fn on_update() {
            record(`${random()} ${random_range(10.0, 20.0)}`);
        }"#;
        let run = |seed: u64| {
            let (mut system, seen) = recording_system();
            system.make_deterministic(seed);
            let mut world = FlintWorld::new();
            let id = world.spawn("dice").unwrap();
            let ast = system.engine.compile(source).unwrap();
            system.engine.add_script(id, ast, "dice.rhai".into());
            for _ in 0..4 {
                system.update(&mut world, 1.0 / 60.0).unwrap();
            }
            let out = seen.lock().unwrap().clone();
            out
        };
        let first = run(7);
        assert_eq!(first, run(7));
        assert_ne!(first, run(8));
        for line in &first {
            let mut parts = line.split(' ').map(|p| p.parse::<f64>().unwrap());
            let (unit, ranged) = (parts.next().unwrap(), parts.next().unwrap());
            assert!((0.0..1.0).contains(&unit), "{unit}");
            assert!((10.0..20.0).contains(&ranged), "{ranged}");
        }
        // Not stuck on one value.
        assert_ne!(first[0], first[1]);
    }

    #[test]
    fn seed_random_is_make_deterministics_sequence_and_keeps_the_clock() {
        // The player's --seed and flint render --scripts draw the same numbers
        let source = r#"fn on_update() {
            record(`${random()} ${random_range(-1.0, 1.0)}`);
            try { let t = timestamp(); record("clock"); }
            catch (e) { record("no clock"); }
        }"#;
        let run = |seeded_only: bool| {
            let (mut system, seen) = recording_system();
            if seeded_only {
                system.seed_random(0x5EED);
            } else {
                system.make_deterministic(0x5EED);
            }
            let mut world = FlintWorld::new();
            let id = world.spawn("dice").unwrap();
            let ast = system.engine.compile(source).unwrap();
            system.engine.add_script(id, ast, "dice.rhai".into());
            for _ in 0..3 {
                system.update(&mut world, 1.0 / 60.0).unwrap();
            }
            let out = seen.lock().unwrap().clone();
            out
        };
        let (player, render) = (run(true), run(false));
        let numbers = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
                .cloned()
                .collect()
        };
        assert_eq!(numbers(&player), numbers(&render));
        assert_eq!(numbers(&player).len(), 3);
        // Only the render takes the wall clock away
        assert!(player.iter().any(|l| l == "clock"), "{player:?}");
        assert!(render.iter().any(|l| l == "no clock"), "{render:?}");
    }

    #[test]
    fn a_script_seeds_random_itself_and_draws_what_the_host_seed_draws() {
        // #580: seed_random(n) in Rhai is --seed n, mid-run as well as at start
        let run = |source: &str, host_seed: Option<u64>| {
            let (mut system, seen) = recording_system();
            if let Some(seed) = host_seed {
                system.seed_random(seed);
            }
            let mut world = FlintWorld::new();
            let id = world.spawn("dice").unwrap();
            let ast = system.engine.compile(source).unwrap();
            system.engine.add_script(id, ast, "dice.rhai".into());
            for _ in 0..3 {
                system.update(&mut world, 1.0 / 60.0).unwrap();
            }
            let out = seen.lock().unwrap().clone();
            out
        };
        let draw = r#"fn on_update() {
            record(`${random()} ${random()} ${random_range(5.0, 6.0)}`);
        }"#;
        // In on_init: top-level statements run again before every callback,
        // so a seed_random(n) up there would restart the sequence each frame
        let seeded_here = r#"
            fn on_init() { seed_random(1234); }
            fn on_update() {
                record(`${random()} ${random()} ${random_range(5.0, 6.0)}`);
            }"#;
        let by_host = run(draw, Some(1234));
        assert_eq!(run(seeded_here, None), by_host);
        assert_eq!(
            run(seeded_here, Some(99)),
            by_host,
            "the script's seed wins"
        );
        // Unseeded runs start from the clock: two engines differ, and no two
        // draws in a frame repeat
        let (a, b) = (run(draw, None), run(draw, None));
        assert_ne!(a, b);
        for line in a.iter().chain(&b) {
            let v: Vec<&str> = line.split(' ').collect();
            assert_ne!(v[0], v[1], "{line}");
        }
        // Negative seeds are a u64's bits
        let neg = run(
            r#"fn on_init() { seed_random(-1); } fn on_update() { record(`${random()}`); }"#,
            None,
        );
        let host = run(
            r#"fn on_update() { record(`${random()}`); }"#,
            Some(u64::MAX),
        );
        assert_eq!(neg, host);
    }

    #[test]
    fn a_setter_on_a_missing_entity_warns_once_per_script_and_function() {
        let source = r#"fn on_update() {
            let gone = get_entity("car_body");
            set_position(gone, 1.0, 2.0, 3.0);
            set_position(gone, 1.0, 2.0, 3.0);
            set_rotation_quat(gone, 0.0, 0.0, 0.0, 1.0);
            set_field(9999, "transform", "position", [0.0, 0.0, 0.0]);
            set_position(self_entity(), 4.0, 5.0, 6.0);
        }"#;
        let mut system = ScriptSystem::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("driver").unwrap();
        world
            .set_field(
                id,
                "script",
                "source",
                toml::Value::String("driver.rhai".into()),
            )
            .unwrap();
        let ast = system.engine.compile(source).unwrap();
        system.engine.add_script(id, ast, "driver.rhai".into());
        for _ in 0..3 {
            system.update(&mut world, 1.0 / 60.0).unwrap();
        }
        let warned = lock_or_recover(&system.engine.ctx)
            .bad_target_warned
            .clone();
        let mut warned: Vec<_> = warned.into_iter().collect();
        warned.sort();
        let named = |f: &'static str| ("driver.rhai".to_string(), f);
        assert_eq!(
            warned,
            vec![
                named("set_field"),
                named("set_position"),
                named("set_rotation_quat")
            ]
        );
        // A real entity is still set, and doesn't warn
        let pos = world.get_transform(id).unwrap().position;
        assert_eq!((pos.x, pos.y, pos.z), (4.0, 5.0, 6.0));
    }

    #[test]
    fn deterministic_runs_have_no_wall_clock() {
        let (mut system, seen) = recording_system();
        system.make_deterministic(1);
        let source = r#"fn on_update() {
            try { let t = timestamp(); record("read the wall clock"); }
            catch (e) { record(`${e}`); }
        }"#;
        let mut world = FlintWorld::new();
        let id = world.spawn("clock").unwrap();
        let ast = system.engine.compile(source).unwrap();
        system.engine.add_script(id, ast, "clock.rhai".into());
        system.update(&mut world, 1.0 / 60.0).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains("total_time()"), "{}", seen[0]);
    }

    #[test]
    fn scripts_run_in_the_order_of_their_entities_names() {
        let (mut system, seen) = recording_system();
        let mut world = FlintWorld::new();
        // Spawned, so numbered, in the opposite order to their names.
        for name in ["c_last", "b_middle", "a_first"] {
            let id = world.spawn(name).unwrap();
            let source = format!(
                r#"fn on_update() {{ record("{name}"); }}
                   fn on_expression(state) {{ record("{name} heard"); }}"#
            );
            let ast = system.engine.compile(&source).unwrap();
            system.engine.add_script(id, ast, format!("{name}.rhai"));
            system.engine.name_script(id, name);
        }
        system.queue_expression(rhai::Map::new());
        system.update(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "a_first heard",
                "b_middle heard",
                "c_last heard",
                "a_first",
                "b_middle",
                "c_last"
            ]
        );
    }

    #[test]
    fn expression_backlog_keeps_the_latest() {
        let mut system = ScriptSystem::new();
        for i in 0..(MAX_PENDING_EXPRESSIONS + 10) {
            let mut m = rhai::Map::new();
            m.insert("i".into(), (i as i64).into());
            system.queue_expression(m);
        }
        assert_eq!(system.pending_expressions.len(), MAX_PENDING_EXPRESSIONS);
        assert_eq!(system.pending_expressions[0]["i"].as_int().unwrap(), 10);
    }

    #[test]
    fn test_desaturation_override_round_trip() {
        let mut system = ScriptSystem::new();
        {
            let mut c = crate::lock_or_recover(&system.engine.ctx);
            c.postprocess_desaturation_override = Some(0.85);
        }
        let overrides = system.take_postprocess_overrides();
        assert_eq!(overrides.desaturation, Some(0.85));
        // Drain clears: a second take returns all-None
        let overrides = system.take_postprocess_overrides();
        assert_eq!(overrides, PostProcessOverrides::default());
    }
}

/// Lock a shared script-state mutex, recovering from poisoning instead of
/// propagating the panic (ADR 0039). The guarded values are always-valid
/// snapshots written whole each frame, so the last-written state is safe to
/// read after a writer panicked; propagating would turn one panic into a
/// panic on every later script call, contradicting the crate's
/// log-and-continue policy. Logs once per process.
pub(crate) fn lock_or_recover<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::error!(
                "script context mutex poisoned (a thread panicked holding it);                  recovering with the last-written snapshot (ADR 0039)"
            );
        }
        poisoned.into_inner()
    })
}
