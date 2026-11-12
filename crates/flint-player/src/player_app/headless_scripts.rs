//! Scripts with no window (#8): what `flint render --scripts` runs, so a
//! headless capture shows what a scene's Rhai behaviors do to it.
//!
//! A tick is a frame of the player's, in the player's order: the expression
//! timeline's due states are queued, scripts update (`on_expression` before
//! `on_update`; not while a pushed state pauses scripts), `on_draw_ui` runs,
//! the style tweens move on, and the commands the scripts left are done. The
//! caller then steps the living props and the wooze field, which is where the
//! player's frame goes next, so a tune lands where it would in the player.
//!
//! It is deterministic: the clock is the caller's fixed step, `random()` is a
//! seeded sequence ([`ScriptSystem::make_deterministic`]), scripts run in the
//! order of their entities' names, and there is no input. Same scene, same
//! timeline, same pixels.
//!
//! The commands go through the same dispatcher as the player's
//! ([`flint_script::host::apply`], #697): a capture is a [`ScriptHost`] too.
//! Everything that changes the picture is done: what a script writes to the
//! world, the entity styles and their tweens, the whole-frame styles, looks,
//! the camera (its roll too), the post-processing overrides, render modes,
//! the distance and quality levers, style-history resets, the wooze field and
//! Living Props, particles, game states and events between scripts. What a
//! capture can't do is [`PRESENTATION_ONLY`], and a test holds every command
//! to being one or the other. Of those, sound and the window's pointer and
//! touch have nothing to show and are dropped quietly; the rest is named
//! once in [`HeadlessScripts::notes`], so a review of the capture knows what
//! it isn't showing.

use std::collections::BTreeSet;

use flint_core::quality::QualitySettings;
use flint_core::wooze::{WoozeField, WoozeSample};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_procgen::{BoilDef, WoozePreset};
use flint_render::PostProcessConfig;
use flint_runtime::{GameEvent, GameStateMachine, InputState, PersistentStore, RuntimeSystem};
use flint_script::context::ScriptCommand;
use flint_script::{CameraOverrides, PostProcessOverrides, ScriptHost, ScriptSystem};

use super::expression_feed::{json_to_map, ExpressionTimeline};
use super::living_props::{LivingRun, WoozeClocks};
use super::living_tuning::{
    boil_problem, clear_species, find_preset, replace_species, tune_boil, tune_field, tune_global,
    tune_props, tune_species, ScriptTune,
};
use super::style_commands::{self, SceneStyles, StyleApplied, StyleTweens};

/// The seed of `random()` in a headless run. One fixed value, so that every
/// render of a scene draws the same numbers.
pub const SCRIPT_SEED: u64 = 0x5EED_0F08;

/// A command a headless capture doesn't do, and why.
#[derive(Debug, Clone, Copy)]
pub struct Skipped {
    /// The script function, as [`ScriptHost::unsupported`] names it.
    pub command: &'static str,
    /// Why a capture can't. `None`: nothing in a picture shows it, so it is
    /// dropped without a note.
    pub why: Option<&'static str>,
}

const fn quiet(command: &'static str) -> Skipped {
    Skipped { command, why: None }
}

const fn noted(command: &'static str, why: &'static str) -> Skipped {
    Skipped {
        command,
        why: Some(why),
    }
}

/// The commands a headless capture leaves to the player: sound, the window's
/// pointer and touch, the clock, files, the A/B view, other scenes, quitting
/// and physics. Every other command is applied headless; a test
/// (`headless_host_tests.rs`) fails on a command that is neither.
pub const PRESENTATION_ONLY: &[Skipped] = &[
    quiet("play_sound"),
    quiet("play_sound_at"),
    quiet("stop_sound"),
    quiet("set_audio_lowpass"),
    quiet("set_touch_controls"),
    quiet("set_cursor_captured"),
    quiet("rumble"),
    noted(
        "set_time_scale",
        "the capture's clock is its own fixed step",
    ),
    noted(
        "set_camera_cut_detection",
        "a capture has no camera-cut detection to switch",
    ),
    noted("save_living_preset", "a capture writes no files"),
    noted("commit_living", "a capture writes no files"),
    noted("compare_living", "the A/B view is the player's window"),
    noted("flip_living", "the A/B view is the player's window"),
    noted("split_living", "the A/B view is the player's window"),
    noted(
        "stop_comparing_living",
        "the A/B view is the player's window",
    ),
    noted("load_scene", "a capture draws the one scene it loaded"),
    noted("reload_scene", "a capture draws the one scene it loaded"),
    noted("load_chunk", "a capture draws the scene as it loaded"),
    noted("unload_chunk", "a capture draws the scene as it loaded"),
    noted("quit_game", "a capture ends at its last frame"),
    noted("set_velocity_2d", "physics doesn't step in a capture"),
];

/// A scene's scripts, run on a clock someone else keeps.
pub struct HeadlessScripts {
    system: ScriptSystem,
    timeline: Option<ExpressionTimeline>,
    // What the player's scripts reach through their state scope.
    state_machine: GameStateMachine,
    store: PersistentStore,
    physics: PhysicsSystem,
    input: InputState,
    /// Events a script fired, for the next tick's scripts, as the player hands
    /// them on through its physics events.
    events: Vec<GameEvent>,
    /// `freeze_living`: the living clock stands still.
    frozen: bool,
    /// `step_living`: whole beats to move the living clock on by, before it's next
    /// read.
    steps: u32,
    /// Boil tunes not yet laid on the renderer's config, oldest first.
    boils: Vec<BoilDef>,
    /// While a script has switched the boil off: the amounts it comes back to.
    boil_off: Option<(f32, f32)>,
    /// Where `load_living_preset` looks for files.
    scene_path: String,
    /// The game time of the last tick.
    time: f64,
    /// What the scripts asked for and didn't get, once each.
    notes: BTreeSet<String>,
    /// The commands of [`PRESENTATION_ONLY`] the scripts asked for.
    skipped: BTreeSet<&'static str>,
    /// Post-processing as the scripts last left it. The renderer's config is
    /// sticky in the player, so only the last write of each value matters.
    post: PostProcessOverrides,
    fog_color: Option<[f32; 3]>,
    /// This tick's render mode (a reality tear lasts only while asked for)
    /// and the mode's tuning, which is held.
    render_mode: Option<(u32, f32)>,
    mode_params: Option<[f32; 4]>,
    /// The camera the scripts asked for in the last tick.
    camera: CameraOverrides,
    screen: (f32, f32),
    /// The entity style tweens (#370), stepped each tick as the player's are.
    style_tweens: StyleTweens,
    /// The post-process config the frame is drawn with, as the stage last
    /// handed it, with the scripts' whole-frame style edits laid on: what
    /// style reads see and tweens start from.
    styles: PostProcessConfig,
    /// The scene's own config, for `reset_post_style`.
    authored: PostProcessConfig,
    /// A whole-frame style changed since the stage last took the styles.
    styles_changed: bool,
    /// `set_look` / `clear_look` since the stage last asked.
    look: Option<Option<String>>,
    /// `set_lod_scale` / `set_draw_distance_scale` since the stage last asked.
    lod: (Option<f32>, Option<f32>),
    /// `set_quality` since the stage last asked.
    quality: Option<QualitySettings>,
    /// `reset_style_history` since the stage last asked.
    reset_history: bool,
}

/// What the scripts have asked of the picture so far.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScriptLook {
    pub post: PostProcessOverrides,
    pub fog_color: Option<[f32; 3]>,
    /// The render mode of the last tick: shown only while asked for.
    pub render_mode: Option<(u32, f32)>,
    pub mode_params: Option<[f32; 4]>,
}

/// What a capture's renderer should take from the scripts before it draws:
/// [`HeadlessScripts::take_renderer_requests`].
#[derive(Debug, Clone, Default)]
pub struct RendererRequests {
    /// The whole-frame styles as the scripts left them, if they changed.
    pub styles: Option<PostProcessConfig>,
    /// A look to lay under the scene's keys (`Some(None)`: none).
    pub look: Option<Option<String>>,
    pub lod_scale: Option<f32>,
    pub draw_distance_scale: Option<f32>,
    pub quality: Option<QualitySettings>,
    pub reset_style_history: bool,
}

impl HeadlessScripts {
    /// Load the scripts of the scene at `scene_path`, found in `project` as
    /// the player finds them, and run their `on_init`, as the player does once
    /// its models are loaded. `screen` is the capture's size, for
    /// `screen_width()` and the like; `post` the post-process config the
    /// capture draws with, for style reads and tweens (it is also the scene's
    /// own, for `reset_post_style`).
    pub fn load(
        scene_path: &str,
        project: &flint_core::project::Project,
        world: &mut FlintWorld,
        timeline: Option<ExpressionTimeline>,
        screen: (f32, f32),
        post: &PostProcessConfig,
    ) -> Self {
        let mut system = ScriptSystem::new();
        system.make_deterministic(SCRIPT_SEED);
        system.load_scripts_from_scene(scene_path, project);
        system.set_current_scene(scene_path, project);
        system.set_screen_size(screen.0, screen.1);
        let mut run = Self {
            system,
            timeline,
            state_machine: super::subsystem::state_machine(),
            store: PersistentStore::new(),
            physics: PhysicsSystem::new(),
            input: InputState::new(),
            events: Vec::new(),
            frozen: false,
            steps: 0,
            boils: Vec::new(),
            boil_off: None,
            scene_path: scene_path.to_string(),
            time: 0.0,
            notes: BTreeSet::new(),
            skipped: BTreeSet::new(),
            post: PostProcessOverrides::default(),
            fog_color: None,
            render_mode: None,
            mode_params: None,
            camera: CameraOverrides::default(),
            screen,
            style_tweens: StyleTweens::default(),
            styles: post.clone(),
            authored: post.clone(),
            styles_changed: false,
            look: None,
            lod: (None, None),
            quality: None,
            reset_history: false,
        };
        run.set_post_config(post);
        {
            let _scope =
                run.system
                    .state_scope(&mut run.state_machine, &mut run.store, &run.physics);
            if let Err(e) = run.system.initialize(world) {
                tracing::warn!("Script init failed: {e:?}");
            }
        }
        run
    }

    /// The post-process config the capture now draws with (after a look, a
    /// boil, the scripts' own styles): what scripts' style reads see and
    /// their tweens start from, as the player's `SceneStyles` reader does.
    pub fn set_post_config(&mut self, config: &PostProcessConfig) {
        self.styles = config.clone();
        self.system
            .set_style_reader(std::sync::Arc::new(SceneStyles::new(config.clone())));
    }

    /// One frame of scripts at game time `time`, `dt` after the last. The camera
    /// is where the frame before left it. Returns the commands the scripts left,
    /// for [`Self::apply`].
    pub fn tick(
        &mut self,
        world: &mut FlintWorld,
        time: f64,
        dt: f64,
        camera: CameraView,
    ) -> Vec<ScriptCommand> {
        self.time = time;
        if let Some(timeline) = &mut self.timeline {
            for due in timeline.due(time) {
                self.system.queue_expression(json_to_map(&due.state));
            }
        }
        let events = std::mem::take(&mut self.events);
        let running = self
            .state_machine
            .active_config()
            .runs(flint_runtime::state::SCRIPTS);
        let _scope =
            self.system
                .state_scope(&mut self.state_machine, &mut self.store, &self.physics);
        self.system.set_transition_state(-1.0, "idle");
        self.system.set_camera(camera.position, camera.forward);
        self.system.set_camera_view_proj(camera.view_proj);
        self.system.provide_context(&self.input, &events, time, dt);
        self.system.set_screen_size(self.screen.0, self.screen.1);
        if running {
            if let Err(e) = self.system.update(world, dt) {
                tracing::warn!("Script error: {e:?}");
            }
        }
        self.system.call_draw_uis(world);
        if !self.system.drain_draw_commands().is_empty() {
            self.note("on_draw_ui: the HUD isn't drawn in a headless capture");
        }
        if !self.system.drain_world_lines().is_empty() {
            self.note("draw_world_line: world lines aren't drawn in a headless capture");
        }
        if !self.system.drain_world_texts().is_empty() {
            self.note("world text isn't drawn in a headless capture");
        }
        // The style tweens move on before this tick's commands, as the
        // player's do: a tween asked for now starts moving next tick.
        if running {
            let step = self.style_tweens.step(world, dt);
            self.events.extend(step.events);
            for message in step.failures {
                tracing::warn!(target: "script", "{message}");
            }
        }
        // The camera and the render mode last the tick that asks
        self.camera = CameraOverrides::default();
        self.render_mode = None;
        self.system.drain_commands()
    }

    /// Do what `commands` ask, through the player's dispatcher: to `world`,
    /// and to `living`, `wooze` and `particles` (the stage's, if it has them)
    /// at the time their clocks have reached, before they step on, where the
    /// player does them. `shallow` is whether a field the scripts give a scene
    /// that had none draws shallow wooze. `particles` is `None` when the
    /// capture draws none. What needs the renderer waits for
    /// [`Self::take_renderer_requests`].
    pub fn apply(
        &mut self,
        commands: Vec<ScriptCommand>,
        world: &mut FlintWorld,
        living: Option<&mut LivingRun>,
        wooze: &mut Option<WoozeClocks>,
        particles: Option<&mut ParticleSystem>,
        shallow: bool,
    ) {
        let mut host = HeadlessHost {
            run: self,
            world,
            living,
            wooze,
            particles,
            shallow,
        };
        flint_script::host::apply_all(&mut host, commands);
    }

    /// A script's `tune_living` or a preset it loads, as the player's: the
    /// field's knobs, then the grove's, then a species', and the boil, which waits
    /// for the capture's post-processing ([`Self::tune_boil_into`]). With
    /// `replace`, as loading a preset does (#249), each species named gets exactly
    /// its values rather than those laid over what it has.
    fn tune(
        &mut self,
        tune: ScriptTune,
        replace: bool,
        mut living: Option<&mut LivingRun>,
        wooze: &mut Option<WoozeClocks>,
        shallow: bool,
    ) -> Result<(), String> {
        let mut problems = Vec::new();
        let no_props = |name: &str| format!("no living prop grows from '{name}'");
        match tune {
            ScriptTune::Grove(tune) => {
                let WoozePreset {
                    wooze: knobs,
                    living: grove,
                    boil,
                } = tune;
                let start = (living.as_deref())
                    .and_then(|living| living.props().props().first())
                    .map_or(WoozeSample::NEUTRAL, |prop| prop.wooze_below());
                match wooze {
                    Some(clocks) => {
                        let (field, time) = clocks.field_mut();
                        problems.extend(tune_global(field, time, &knobs));
                    }
                    None => {
                        // A scene with no [wooze] table: the tune may give it a
                        // field, at the time the props have reached (or the
                        // scripts', with no props).
                        let time = living.as_deref().map_or(self.time, LivingRun::time);
                        let mut slot = None;
                        problems.extend(tune_field(&mut slot, start, time, &knobs));
                        if let Some(field) = slot {
                            *wooze = Some(WoozeClocks::of_field(field, time, shallow));
                        }
                    }
                }
                if let Some(living) = living.as_deref_mut() {
                    problems.extend(tune_props(living.props_mut(), &grove));
                }
                for (name, species) in &grove.species {
                    let grown = match living.as_deref_mut() {
                        Some(living) => {
                            let (said, grown) = if replace {
                                replace_species(living.props_mut(), name, species)
                            } else {
                                tune_species(living.props_mut(), name, species)
                            };
                            problems.extend(said);
                            grown
                        }
                        None => false,
                    };
                    if !grown {
                        problems.push(no_props(name));
                    }
                }
                if let Some(boil) = boil {
                    problems.extend(boil_problem(&boil));
                    self.boils.push(boil);
                }
            }
            ScriptTune::Species(name, def) => {
                let Some(living) = living else {
                    return Err(no_props(&name));
                };
                let (said, grown) = tune_species(living.props_mut(), &name, &def);
                problems.extend(said);
                if !grown {
                    problems.push(no_props(&name));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// The particles a command goes to, or a note that the capture draws none.
    fn particles<'a>(
        &mut self,
        particles: Option<&'a mut ParticleSystem>,
    ) -> Option<&'a mut ParticleSystem> {
        if particles.is_none() {
            self.note(
                "particles: a still with no --time draws none; give it --time or \
                 --particle-time to see what the scripts emit",
            );
        }
        particles
    }

    /// One change to the stage's field, at the time its clock has reached.
    fn change_wooze(
        &mut self,
        wooze: &mut Option<WoozeClocks>,
        change: &dyn Fn(&mut WoozeField, f64) -> Result<(), String>,
    ) {
        let Some(clocks) = wooze else {
            self.note(
                "set_wooze: this scene has no wooze field; give it a [wooze] table, even an \
                 empty one, to opt in",
            );
            return;
        };
        let (field, time) = clocks.field_mut();
        if let Err(reason) = change(field, time) {
            tracing::warn!("A script's change to the wooze field: {reason}");
        }
    }

    /// A style command's outcome: its event to the next tick's scripts, its
    /// refusal to the log.
    fn style_applied(&mut self, applied: StyleApplied) {
        match applied {
            StyleApplied::Done(Some(event)) => self.events.push(event),
            StyleApplied::Done(None) => {}
            StyleApplied::Failed(message) => tracing::warn!(target: "script", "{message}"),
        }
    }

    /// A whole-frame style edit, to the styles the capture draws with.
    fn edit_post_styles(&mut self, edit: impl FnOnce(&mut PostProcessConfig) -> StyleApplied) {
        let mut config = self.styles.clone();
        let applied = edit(&mut config);
        if let StyleApplied::Done(_) = applied {
            self.styles = config;
            self.styles_changed = true;
        }
        self.style_applied(applied);
    }

    fn note(&mut self, what: &str) {
        if self.notes.insert(what.to_string()) {
            tracing::warn!("{what}");
        }
    }

    /// The whole beats `step_living` asked for since the last call, to move the
    /// living clock on by before it's next read.
    pub fn take_steps(&mut self) -> u32 {
        std::mem::take(&mut self.steps)
    }

    /// Lay the boil tunes the scripts have made since the last call on `config`,
    /// in order, as the player's `tune_boil` does to its renderer's. Returns
    /// whether there were any.
    pub fn tune_boil_into(&mut self, config: &mut flint_render::PostProcessConfig) -> bool {
        let tuned = !self.boils.is_empty();
        for boil in self.boils.drain(..) {
            tune_boil(config, &mut self.boil_off, boil);
        }
        tuned
    }

    /// What the scripts have asked of the renderer since the last call: the
    /// whole-frame styles if they changed (a config whose `styles` are what
    /// to draw with), a look, the distance and quality levers, a
    /// style-history reset. The stage lays them on before it draws, then
    /// hands back the config with [`Self::set_post_config`] and what it
    /// applied with [`Self::set_renderer_state`].
    pub fn take_renderer_requests(&mut self) -> RendererRequests {
        let (lod_scale, draw_distance_scale) = std::mem::take(&mut self.lod);
        RendererRequests {
            styles: std::mem::take(&mut self.styles_changed).then(|| self.styles.clone()),
            look: self.look.take(),
            lod_scale,
            draw_distance_scale,
            quality: self.quality.take(),
            reset_style_history: std::mem::take(&mut self.reset_history),
        }
    }

    /// The distance scales and quality the capture draws with, for scripts'
    /// `lod_scale()`, `draw_distance_scale()` and `get_quality()`.
    pub fn set_renderer_state(&mut self, lod: (f32, f32), quality: Option<QualitySettings>) {
        self.system.set_lod_state(lod.0, lod.1);
        if let Some(quality) = quality {
            self.system.set_quality_state(quality);
        }
    }

    /// Whether a script has stopped the living clock (`freeze_living`).
    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// The camera the scripts asked for in the last tick (`set_camera_*`).
    pub fn camera(&self) -> CameraOverrides {
        self.camera
    }

    /// Post-processing and fog as the scripts have left them, and the last
    /// tick's render mode.
    pub fn look(&self) -> ScriptLook {
        ScriptLook {
            post: self.post,
            fog_color: self.fog_color,
            render_mode: self.render_mode,
            mode_params: self.mode_params,
        }
    }

    /// What the scripts asked for that a headless capture doesn't do, once each.
    pub fn notes(&self) -> Vec<String> {
        self.notes.iter().cloned().collect()
    }

    /// The commands of [`PRESENTATION_ONLY`] the scripts asked for, by name.
    pub fn skipped(&self) -> Vec<&'static str> {
        self.skipped.iter().copied().collect()
    }

    /// The expression state scripts see now.
    pub fn expression(&self) -> rhai::Map {
        self.system.expression()
    }
}

/// A tick's commands meeting the capture: [`HeadlessScripts`] and the parts
/// of the stage they reach.
struct HeadlessHost<'a> {
    run: &'a mut HeadlessScripts,
    world: &'a mut FlintWorld,
    living: Option<&'a mut LivingRun>,
    wooze: &'a mut Option<WoozeClocks>,
    particles: Option<&'a mut ParticleSystem>,
    shallow: bool,
}

impl ScriptHost for HeadlessHost<'_> {
    fn host_name(&self) -> &'static str {
        "a headless capture"
    }

    fn unsupported(&mut self, command: &'static str) {
        let run = &mut *self.run;
        run.skipped.insert(command);
        match PRESENTATION_ONLY.iter().find(|s| s.command == command) {
            Some(Skipped { why: None, .. }) => {}
            Some(Skipped { why: Some(why), .. }) => {
                run.note(&format!(
                    "{command}: a headless capture doesn't do it ({why})"
                ));
            }
            None => run.note(&format!(
                "{command}: a script asked for it, and a headless capture doesn't do it"
            )),
        }
    }

    fn fire_event(&mut self, name: String, data: toml::Value) {
        if name != flint_core::events::TRANSITION_COMPLETE {
            self.run.events.push(GameEvent::Custom { name, data });
        }
    }

    fn emit_burst(&mut self, entity: EntityId, count: i64) {
        if let Some(particles) = self.run.particles(self.particles.as_deref_mut()) {
            particles.sync.queue_burst(entity, count.max(0) as u32);
        }
    }

    fn play_effect(&mut self, handle: u64, name: String, position: (f64, f64, f64)) {
        if let Some(particles) = self.run.particles(self.particles.as_deref_mut()) {
            let at = [position.0 as f32, position.1 as f32, position.2 as f32];
            if !particles.sync.spawn_effect(handle, &name, at) {
                tracing::warn!(
                    target: "script",
                    "play_effect: unknown particle effect '{name}' (expected \
                     particles/{name}.particles.toml)"
                );
            }
        }
    }

    fn stop_effect(&mut self, handle: u64) {
        if let Some(particles) = self.run.particles(self.particles.as_deref_mut()) {
            particles.sync.stop_effect(handle);
        }
    }

    fn set_effect_param(&mut self, handle: u64, param: String, value: f64) {
        if let Some(particles) = self.run.particles(self.particles.as_deref_mut()) {
            if !particles.sync.set_effect_param(handle, &param, value) {
                tracing::warn!(
                    target: "script",
                    "set_effect_param: unknown handle {handle} or param '{param}'"
                );
            }
        }
    }

    fn set_entity_style(&mut self, entity: EntityId, style: String) {
        let applied = self
            .run
            .style_tweens
            .set_entity_style(self.world, entity, &style);
        self.run.style_applied(applied);
    }

    fn set_style_param(&mut self, entity: EntityId, param: String, value: toml::Value) {
        let applied = self
            .run
            .style_tweens
            .set_style_param(self.world, entity, &param, value);
        self.run.style_applied(applied);
    }

    fn tween_style_param(
        &mut self,
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        easing: String,
    ) {
        let run = &mut *self.run;
        let applied = run.style_tweens.tween(
            self.world,
            &run.styles,
            entity,
            &param,
            &target,
            seconds,
            &easing,
        );
        run.style_applied(applied);
    }

    fn clear_entity_style(&mut self, entity: EntityId) {
        let applied = self.run.style_tweens.clear_entity_style(self.world, entity);
        self.run.style_applied(applied);
    }

    fn set_post_style(&mut self, style: String, enabled: bool) {
        self.run
            .edit_post_styles(|config| style_commands::set_post_style(config, &style, enabled));
    }

    fn set_post_style_param(&mut self, style: String, param: String, value: toml::Value) {
        self.run.edit_post_styles(|config| {
            style_commands::set_post_style_param(config, &style, &param, value)
        });
    }

    fn reset_post_style(&mut self, style: String) {
        let authored = self.run.authored.clone();
        self.run
            .edit_post_styles(|config| style_commands::reset_post_style(config, &authored, &style));
    }

    fn reset_style_history(&mut self) {
        self.run.reset_history = true;
    }

    fn set_look(&mut self, name: Option<String>) {
        self.run.look = Some(name);
    }

    fn set_camera(&mut self, camera: CameraOverrides) {
        self.run.camera.merge(camera);
    }

    fn set_post_process(&mut self, post: PostProcessOverrides) {
        self.run.post.merge(post);
    }

    fn set_fog_color(&mut self, color: [f32; 3]) {
        self.run.fog_color = Some(color);
    }

    fn set_render_mode(&mut self, mode: Option<(u32, f32)>, params: Option<[f32; 4]>) {
        let run = &mut *self.run;
        run.render_mode = mode.or(run.render_mode);
        run.mode_params = params.or(run.mode_params);
    }

    fn set_lod_scales(&mut self, lod_scale: Option<f32>, draw_distance_scale: Option<f32>) {
        let lod = &mut self.run.lod;
        lod.0 = lod_scale.or(lod.0);
        lod.1 = draw_distance_scale.or(lod.1);
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.run.quality = Some(settings);
    }

    fn set_time_scale(&mut self, scale: f64) {
        // A scale of 1 asks for nothing a capture lacks
        if scale != 1.0 {
            self.unsupported("set_time_scale");
        }
    }

    fn set_wooze(&mut self, knobs: toml::Value, seconds: f64) {
        self.run.change_wooze(self.wooze, &|field, time| {
            field.set_global(table_of(&knobs)?, time, seconds)
        });
    }

    fn set_wooze_zone(&mut self, name: String, fields: toml::Value, seconds: f64) {
        self.run.change_wooze(self.wooze, &|field, time| {
            field
                .set_zone(&name, table_of(&fields)?, time, seconds)
                .map_err(|e| format!("zone '{name}': {e}"))
        });
    }

    fn remove_wooze_zone(&mut self, name: String, seconds: f64) {
        self.run.change_wooze(self.wooze, &|field, time| {
            if field.remove_zone(&name, time, seconds) {
                Ok(())
            } else {
                Err(format!("there's no zone named '{name}' to remove"))
            }
        });
    }

    fn tune_living(&mut self, knobs: toml::Value) {
        let (living, wooze, shallow) = (self.living.as_deref_mut(), &mut *self.wooze, self.shallow);
        let tuned = ScriptTune::of(knobs)
            .and_then(|tune| self.run.tune(tune, false, living, wooze, shallow));
        if let Err(problem) = tuned {
            tracing::warn!(target: "script", "tune_living: {problem}");
        }
    }

    fn clear_living(&mut self, species: String, keys: Vec<String>) {
        let cleared = match self.living.as_deref_mut() {
            Some(living) => clear_species(living.props_mut(), &species, &keys),
            None => Ok(false),
        };
        let problem = match cleared {
            Ok(true) => return,
            Ok(false) => format!("no living prop grows from '{species}'"),
            Err(problem) => problem,
        };
        tracing::warn!(target: "script", "clear_living: {problem}");
    }

    fn freeze_living(&mut self, frozen: bool) {
        self.run.frozen = frozen;
    }

    fn step_living(&mut self) {
        self.run.steps += 1;
    }

    fn load_living_preset(&mut self, name: String) {
        let (living, wooze, shallow) = (self.living.as_deref_mut(), &mut *self.wooze, self.shallow);
        let run = &mut *self.run;
        let loaded = find_preset(std::path::Path::new(&run.scene_path), &name)
            .and_then(|preset| run.tune(ScriptTune::Grove(preset), true, living, wooze, shallow));
        if let Err(problem) = loaded {
            tracing::warn!(target: "script", "load_living_preset: {problem}");
        }
    }

    fn push_state(&mut self, name: String) {
        if !self.run.state_machine.push_state(&name) {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }

    fn pop_state(&mut self) {
        self.run.state_machine.pop_state();
    }

    fn replace_state(&mut self, name: String) {
        if !self.run.state_machine.replace_state(&name) {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }
}

/// Where the camera is for a tick's scripts.
#[derive(Debug, Clone, Copy)]
pub struct CameraView {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub view_proj: [[f32; 4]; 4],
}

/// Lay `look` over a post-process config, as the player's frame does with its
/// scripts' overrides. `before` is the look laid last time, so a render mode
/// that has stopped being asked for is taken off, as the player's is.
pub fn apply_look(config: &mut PostProcessConfig, look: &ScriptLook, before: &ScriptLook) {
    let post = &look.post;
    if let Some(v) = post.vignette {
        config.vignette_enabled = v > 0.001;
        config.vignette_intensity = v;
    }
    if let Some(b) = post.bloom {
        config.bloom_intensity = b;
    }
    if let Some(e) = post.exposure {
        config.exposure = e;
    }
    if let Some(ca) = post.chromatic_aberration {
        config.chromatic_aberration = ca;
    }
    if let Some(rb) = post.radial_blur {
        config.radial_blur = rb;
    }
    if let Some(si) = post.ssao_intensity {
        config.ssao_intensity = si;
    }
    if let Some(fd) = post.fog_density {
        config.fog_density = fd;
    }
    if let Some(d) = post.desaturation {
        config.desaturate = d;
    }
    if let Some(s) = post.dof_strength {
        config.dof_strength = s;
    }
    if let Some(fd) = post.dof_focus_distance {
        config.dof_focus_distance = fd;
    }
    if let Some(fr) = post.dof_focus_range {
        config.dof_focus_range = fr;
    }
    if let Some(fc) = look.fog_color {
        config.fog_color = fc;
    }
    // A reality tear: transient, unlike the sticky overrides above
    match look.render_mode {
        Some((mode, mix)) => {
            config.render_mode = mode;
            config.mode_mix = mix;
        }
        None if before
            .render_mode
            .is_some_and(|(mode, mix)| mode != 0 && mix > 0.0) =>
        {
            config.render_mode = 0;
            config.mode_mix = 0.0;
        }
        None => {}
    }
    if let Some(mp) = look.mode_params {
        config.mode_params = mp;
    }
}

fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} must be a map"))
}

#[cfg(test)]
#[path = "headless_host_tests.rs"]
mod tests;
