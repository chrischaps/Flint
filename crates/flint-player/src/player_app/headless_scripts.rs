//! Scripts with no window (#8): what `flint render --scripts` runs, so a
//! headless capture shows what a scene's Rhai behaviors do to it.
//!
//! A tick is a frame of the player's, in the player's order: the expression
//! timeline's due states are queued, scripts update (`on_expression` before
//! `on_update`), `on_draw_ui` runs, and the commands they left are done. The
//! caller then steps the living props and the wooze field, which is where the
//! player's frame goes next, so a tune lands where it would in the player.
//!
//! It is deterministic: the clock is the caller's fixed step, `random()` is a
//! seeded sequence ([`ScriptSystem::make_deterministic`]), scripts run in the
//! order of their entities' names, and there is no input. Same scene, same
//! timeline, same pixels.
//!
//! It is not the player. What changes the picture runs: everything a script
//! writes to the world, `tune_living`, `freeze_living`, `set_wooze`,
//! `set_wooze_zone`, `remove_wooze_zone`, events between scripts, and the
//! camera and post-processing overrides. Sound has nothing to show and is
//! dropped quietly. Anything else a script asks for (physics, particles, a
//! scene change, the HUD) is named once in [`HeadlessScripts::notes`], so a
//! review of the capture knows what it isn't showing.

use std::collections::BTreeSet;

use flint_core::wooze::{WoozeField, WoozeSample};
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use flint_runtime::{GameEvent, GameStateMachine, InputState, PersistentStore, RuntimeSystem};
use flint_script::context::{LogLevel, ScriptCommand};
use flint_script::{CameraOverrides, PostProcessOverrides, ScriptSystem};

use super::expression_feed::{json_to_map, ExpressionTimeline};
use super::living_props::{LivingRun, WoozeClocks};
use super::living_tuning::{tune_field, tune_props, tune_species, ScriptTune};

/// The seed of `random()` in a headless run. One fixed value, so that every
/// render of a scene draws the same numbers.
pub const SCRIPT_SEED: u64 = 0x5EED_0F08;

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
    /// What the scripts asked for and didn't get, once each.
    notes: BTreeSet<String>,
    /// Post-processing as the scripts last left it. The renderer's config is
    /// sticky in the player, so only the last write of each value matters.
    post: PostProcessOverrides,
    fog_color: Option<[f32; 3]>,
    camera: CameraOverrides,
    screen: (f32, f32),
}

/// What the scripts have asked of the picture so far.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScriptLook {
    pub post: PostProcessOverrides,
    pub fog_color: Option<[f32; 3]>,
}

impl HeadlessScripts {
    /// Load the scripts of the scene at `scene_path` from its `scripts/` folder
    /// and run their `on_init`, as the player does once its models are loaded.
    /// `screen` is the capture's size, for `screen_width()` and the like.
    pub fn load(
        scene_path: &str,
        world: &mut FlintWorld,
        timeline: Option<ExpressionTimeline>,
        screen: (f32, f32),
    ) -> Self {
        let mut system = ScriptSystem::new();
        system.make_deterministic(SCRIPT_SEED);
        system.load_scripts_from_scene(scene_path);
        system.set_current_scene(scene_path);
        system.set_screen_size(screen.0, screen.1);
        let mut run = Self {
            system,
            timeline,
            state_machine: GameStateMachine::new(),
            store: PersistentStore::new(),
            physics: PhysicsSystem::new(),
            input: InputState::new(),
            events: Vec::new(),
            frozen: false,
            notes: BTreeSet::new(),
            post: PostProcessOverrides::default(),
            fog_color: None,
            camera: CameraOverrides::default(),
            screen,
        };
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
        if let Some(timeline) = &mut self.timeline {
            for due in timeline.due(time) {
                self.system.queue_expression(json_to_map(&due.state));
            }
        }
        let events = std::mem::take(&mut self.events);
        let _scope =
            self.system
                .state_scope(&mut self.state_machine, &mut self.store, &self.physics);
        self.system.set_transition_state(-1.0, "idle");
        self.system.set_camera(camera.position, camera.forward);
        self.system.set_camera_view_proj(camera.view_proj);
        self.system.provide_context(&self.input, &events, time, dt);
        self.system.set_screen_size(self.screen.0, self.screen.1);
        if let Err(e) = self.system.update(world, dt) {
            tracing::warn!("Script error: {e:?}");
        }
        if let Some(scale) = self.system.take_time_scale_request() {
            if scale != 1.0 {
                self.note("set_time_scale: the capture's clock is its own fixed step");
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
        let camera = self.system.take_camera_overrides();
        self.camera = camera;
        if camera.roll.is_some() {
            self.note("camera roll: the capture's camera stays level");
        }
        let post = self.system.take_postprocess_overrides();
        merge_post(&mut self.post, post);
        if let Some(color) = self.system.take_fog_color_override() {
            self.fog_color = Some(color);
        }
        let (mode, params) = self.system.take_render_mode_overrides();
        if mode.is_some() || params.is_some() {
            self.note("render modes (reality tears) don't run in a headless capture");
        }
        self.system.drain_commands()
    }

    /// Do what `commands` ask that a capture can show, to `living` and `wooze`
    /// (the stage's, if it has them) at the time their clocks have reached,
    /// before they step on: where the player does them. `shallow` is whether a
    /// field the scripts give a scene that had none draws shallow wooze.
    pub fn apply(
        &mut self,
        commands: Vec<ScriptCommand>,
        mut living: Option<&mut LivingRun>,
        wooze: &mut Option<WoozeClocks>,
        shallow: bool,
    ) {
        for command in commands {
            match command {
                ScriptCommand::Log { level, message } => match level {
                    LogLevel::Info => tracing::info!(target: "script", "{}", message),
                    LogLevel::Warn => tracing::warn!(target: "script", "{}", message),
                    LogLevel::Error => tracing::error!(target: "script", "{}", message),
                },
                ScriptCommand::FireEvent { name, data } => {
                    if name != flint_core::events::TRANSITION_COMPLETE {
                        self.events.push(GameEvent::Custom { name, data });
                    }
                }
                ScriptCommand::SetWooze { knobs, seconds } => {
                    self.change_wooze(living.as_deref_mut(), wooze, &|field, time| {
                        field.set_global(table_of(&knobs)?, time, seconds)
                    });
                }
                ScriptCommand::SetWoozeZone {
                    name,
                    fields,
                    seconds,
                } => {
                    self.change_wooze(living.as_deref_mut(), wooze, &|field, time| {
                        field
                            .set_zone(&name, table_of(&fields)?, time, seconds)
                            .map_err(|e| format!("zone '{name}': {e}"))
                    });
                }
                ScriptCommand::RemoveWoozeZone { name, seconds } => {
                    self.change_wooze(living.as_deref_mut(), wooze, &|field, time| {
                        if field.remove_zone(&name, time, seconds) {
                            Ok(())
                        } else {
                            Err(format!("there's no zone named '{name}' to remove"))
                        }
                    });
                }
                ScriptCommand::TuneLiving { knobs } => {
                    if let Err(problem) = self.tune(knobs, living.as_deref_mut(), wooze, shallow) {
                        tracing::warn!(target: "script", "tune_living: {problem}");
                    }
                }
                ScriptCommand::FreezeLiving { frozen } => self.frozen = frozen,
                ScriptCommand::PlaySound { .. }
                | ScriptCommand::PlaySoundAt { .. }
                | ScriptCommand::StopSound { .. } => {}
                other => {
                    let name = format!("{other:?}");
                    let name = name
                        .split(|c: char| !c.is_alphanumeric())
                        .next()
                        .unwrap_or("a command")
                        .to_string();
                    self.note(&format!(
                        "{name}: a script asked for it, and a headless capture doesn't do it"
                    ));
                }
            }
        }
    }

    /// A script's `tune_living`, as the player's: the field's knobs, then the
    /// grove's, then a species'. The boil is the post-process chain's, which
    /// this doesn't reach yet.
    fn tune(
        &mut self,
        knobs: toml::Value,
        living: Option<&mut LivingRun>,
        wooze: &mut Option<WoozeClocks>,
        shallow: bool,
    ) -> Result<(), String> {
        let Some(living) = living else {
            return Err("this scene has no living props to tune".to_string());
        };
        let mut problems = Vec::new();
        match ScriptTune::of(knobs)? {
            ScriptTune::Grove(tune) => {
                let start = (living.props().props().first())
                    .map_or(WoozeSample::NEUTRAL, |prop| prop.wooze_below());
                let time = living.time();
                problems.extend(tune_field(living.field_slot(), start, time, &tune.wooze));
                match wooze {
                    Some(clocks) => clocks.change_field(|field, time| {
                        tune_field(field, start, time, &tune.wooze);
                    }),
                    None => {
                        // The tune gave a scene with no [wooze] table a field.
                        if let Some(field) = living.field() {
                            *wooze = Some(WoozeClocks::of_field(field.clone(), time, shallow));
                        }
                    }
                }
                problems.extend(tune_props(living.props_mut(), &tune.living));
                for (name, species) in &tune.living.species {
                    let (said, grown) = tune_species(living.props_mut(), name, species);
                    problems.extend(said);
                    if !grown {
                        problems.push(format!("no living prop grows from '{name}'"));
                    }
                }
                if tune.boil.is_some() {
                    self.note("tune_living's boil: a headless capture keeps the scene's");
                }
            }
            ScriptTune::Species(name, def) => {
                let (said, grown) = tune_species(living.props_mut(), &name, &def);
                problems.extend(said);
                if !grown {
                    problems.push(format!("no living prop grows from '{name}'"));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// One change to the field, made the same way to the living props' copy and
    /// the other clocks', which the player keep as one.
    fn change_wooze(
        &mut self,
        living: Option<&mut LivingRun>,
        wooze: &mut Option<WoozeClocks>,
        change: &dyn Fn(&mut WoozeField, f64) -> Result<(), String>,
    ) {
        let mut problem = None;
        let mut changed = false;
        if let Some(living) = living {
            let time = living.time();
            if let Some(field) = living.field_slot() {
                changed = true;
                problem = change(field, time).err();
            }
        }
        if let Some(clocks) = wooze {
            changed = true;
            clocks.change_field(|field, time| {
                if let Some(field) = field {
                    let said = change(field, time).err();
                    problem = problem.take().or(said);
                }
            });
        }
        if !changed {
            self.note(
                "set_wooze: this scene has no wooze field; give it a [wooze] table, even an \
                 empty one, to opt in",
            );
        } else if let Some(reason) = problem {
            tracing::warn!("A script's change to the wooze field: {reason}");
        }
    }

    fn note(&mut self, what: &str) {
        if self.notes.insert(what.to_string()) {
            tracing::warn!("{what}");
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

    /// Post-processing and fog as the scripts have left them.
    pub fn look(&self) -> ScriptLook {
        ScriptLook {
            post: self.post,
            fog_color: self.fog_color,
        }
    }

    /// What the scripts asked for that a headless capture doesn't do, once each.
    pub fn notes(&self) -> Vec<String> {
        self.notes.iter().cloned().collect()
    }

    /// The expression state scripts see now.
    pub fn expression(&self) -> rhai::Map {
        self.system.expression()
    }
}

/// Where the camera is for a tick's scripts.
#[derive(Debug, Clone, Copy)]
pub struct CameraView {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub view_proj: [[f32; 4]; 4],
}

/// Lay this tick's overrides over the ones before: what was set stays set.
fn merge_post(into: &mut PostProcessOverrides, from: PostProcessOverrides) {
    macro_rules! keep {
        ($($field:ident),*) => {
            $(if from.$field.is_some() { into.$field = from.$field; })*
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

/// Lay `look` over a post-process config, as the player's frame does with its
/// scripts' overrides.
pub fn apply_look(config: &mut flint_render::PostProcessConfig, look: &ScriptLook) {
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
}

fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} must be a map"))
}
