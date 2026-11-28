//! The game with no window (ADR 0117 decision 4, #725): the world, its
//! clock, scripts, game states, the persistent store, physics, animation,
//! particles, style tweens and the subsystems, and the one frame order that
//! moves them on, [`step`].
//!
//! `PlayerApp` holds a `GameCore` and adds the window, the renderer, sound,
//! the HUD and the debug tools. It drives a frame by calling [`step`] with
//! itself as the [`FrameHost`]: the step runs the stages in order and calls
//! the host back where the presentation has its part in the frame (a scene
//! transition and extensions before the events, the window's state for the
//! scripts, sound, the HUD's draw lists, the renderer's sync). A host with
//! no window leaves those hooks as they are, so every stage that moves the
//! game runs the same code in both.
//!
//! **What a host lends.** The input the frame reads, the camera, and the
//! renderer and device when it has them ([`Lent`]): a stage that draws
//! (shallow wooze, a living prop's poses, the procgen queue's meshes) uses
//! them, and does without when there are none.

use super::perf::Phase;
use super::style_commands::StyleApplied;
use super::subsystem::{FixedCtx, Stage, StageCtx, Subsystems};
use flint_core::project::Project;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::surfaces::MadeTriangles;
use flint_render::{Camera, SceneRenderer};
use flint_runtime::state::{PHYSICS, SCRIPTS};
use flint_runtime::{
    EventBus, Eye, GameClock, GameEvent, GameStateMachine, InputState, PersistentStore, PlayerView,
    RuntimeSystem, StateConfig,
};
use flint_script::context::ScriptCommand;
use flint_script::{CameraOverrides, PostProcessOverrides, ScriptHost, ScriptSystem};

/// The game's state and systems, with no window (ADR 0117 decision 4).
pub struct GameCore {
    /// The optional parts a scene uses (ADR 0117): torn down last added
    /// first, and declared first so they go before anything else here, and
    /// with the core before the player's audio (ADR 0017).
    pub(crate) subsystems: Subsystems,
    pub world: FlintWorld,
    pub scene_path: String,
    /// The project the current scene belongs to (ADR 0114): what every
    /// asset search starts from.
    pub project: Project,
    pub clock: GameClock,
    /// The frame's game events (#727): physics pushes its contacts at each
    /// fixed step, extensions, style tweens and scripts' `fire_event` their
    /// own, and the frame drains them before scripts run. Physics' `clear`
    /// at a transition leaves it alone.
    pub(crate) events: EventBus,

    pub script: ScriptSystem,
    /// Game states and the persistent store survive scene transitions. The
    /// machine knows the built-in systems and every scene subsystem's
    /// policy name (`subsystem::state_machine`, #727).
    pub(crate) state_machine: GameStateMachine,
    pub(crate) store: PersistentStore,
    /// The styles the scene is drawn with (#742): the built-ins, the
    /// game's (`PlayerExtension::styles`, #419) and the user styles the
    /// scene can see. The core's own, so a headless run has them too; the
    /// renderer is handed a copy at each scene's bind and after a reload.
    pub styles: flint_render::stylize::StyleRegistry,
    /// Entity style tweens (#370), stepped with game time.
    pub(crate) style_tweens: super::style_commands::StyleTweens,
    /// The renderer's config generation the scripts' style reader was
    /// last made from (#372).
    pub(crate) style_reader_generation: Option<u64>,
    /// The run's resources (ADR 0137 decision 2, #1550): what one package
    /// lends another, which every subsystem context lends. What each listed
    /// package puts in (`Registry::run_resources`: the procgen package's
    /// resolver, with the scene's specs and generators and the frame budget
    /// its queue lends the living props, `flint_procgen::player::resolver`);
    /// with `living`, the scene's living props (flint-living, #1024: added
    /// at load, before the living subsystem exists, and kept across scenes
    /// for their pose cache: [`Self::living_props`]); in the player with
    /// `debug-hud`, the F3/F4 panels (`flint_debug_ui::DebugPanels`).
    pub(crate) resources: flint_package::Resources,
    /// What scripts asked of the frame (#697): the camera, post-processing,
    /// distance levers, cursor and touch, held from the command that asked
    /// to the step of the frame that applies each.
    pub(crate) script_requests: flint_script::FrameRequests,
    /// Script commands no part of this scene takes that have been warned
    /// about, once a run each (`set_wooze` in a scene with no field).
    pub(crate) commands_warned: std::collections::HashSet<&'static str>,
    /// The fixed physics steps that ran last frame (the frame history,
    /// #1076); a step a paused state skipped isn't one.
    pub(crate) fixed_steps: u32,
    /// The script modules the game's extensions declared (#754): enabled
    /// for the whole run, beside the subsystems'.
    pub(crate) extension_modules: Vec<&'static str>,
    /// Every named event of the run with its game time (`fire_event` and
    /// the engine's own), for an input script's `expect event` / `wait
    /// event` (#714). `None`, and nothing kept, unless the run has checks.
    pub event_log: Option<Vec<(f64, String)>>,
    /// The packages the project's `[packages]` lets in (#1334), as the
    /// schema directories were named under it: a package it leaves out
    /// runs no subsystem and enables no script module here, as `flint
    /// check` calls its parts unlisted (#1528). `None`: every package this
    /// build has is in.
    pub(crate) packages: Option<flint_core::package::InUse>,
}

impl GameCore {
    /// A core for `world`, loaded from `scene_path`, with nothing started.
    pub fn new(world: FlintWorld, scene_path: String, project: Project) -> Self {
        let mut resources = flint_package::Resources::new();
        crate::packages::registry().make_resources(&mut resources);
        // Who the game is played as (ADR 0138), there from the start so
        // every reader finds one
        resources.insert(PlayerView::default());
        // The triangles of the meshes the asset makers make, by name, for
        // the colliders that name them (#1222): the physics package reads
        // them, the kernel only keeps them (#1617)
        resources.insert(MadeTriangles::default());
        Self {
            subsystems: Default::default(),
            world,
            scene_path,
            project,
            clock: GameClock::new(),
            events: EventBus::new(),

            script: crate::script_system(),
            state_machine: super::subsystem::state_machine(),
            store: PersistentStore::new(),
            styles: Default::default(),
            style_tweens: Default::default(),
            style_reader_generation: None,
            resources,
            script_requests: Default::default(),
            commands_warned: Default::default(),
            fixed_steps: 0,
            extension_modules: Vec::new(),
            event_log: None,
            packages: None,
        }
    }
}

impl GameCore {
    /// Tell the scripts which modules are enabled (#698): those the
    /// subsystems here own and those the game's extensions declared (#754),
    /// so a call into any other is "not enabled". Called whenever either
    /// list changes: a scene's subsystems built (before its scripts load) or
    /// dropped, an extension added.
    ///
    /// A module of a package the project's `[packages]` leaves out is never
    /// enabled (#1528): its subsystem isn't built, and this holds for an
    /// extension that declares one too.
    pub(crate) fn enable_script_modules(&mut self) {
        let mut modules = self.subsystems.script_modules();
        let registry = crate::packages::registry();
        for name in registry.modules_everywhere(self.packages.as_ref()) {
            if !modules.contains(&name) {
                modules.push(name);
            }
        }
        for name in &self.extension_modules {
            if !modules.contains(name) {
                modules.push(name);
            }
        }
        if let Some(in_use) = &self.packages {
            modules.retain(|name| {
                in_use
                    .left_out_owner(flint_core::package::Kind::ScriptModule, name)
                    .is_none()
            });
        }
        self.script.set_enabled_modules(modules);
    }

    /// Whether the project's `[packages]` leaves `package` out (#1528). With
    /// no `[packages]` table nothing is left out.
    pub(crate) fn leaves_out(&self, package: &str) -> bool {
        self.packages
            .as_ref()
            .is_some_and(|in_use| !in_use.is_on(package))
    }

    /// The run's asset makers less those of a package the project's
    /// `[packages]` leaves out (#1578), in the order registered: what every
    /// moment of a made asset asks (the scene, spawned entities, the
    /// frame, `pending`, the teardown), so a left-out package makes
    /// nothing and its names are the file loader's.
    pub(crate) fn asset_makers(&self) -> Vec<&'static dyn flint_package::assets::AssetMaker> {
        crate::packages::registry()
            .asset_makers_in(self.packages.as_ref())
            .collect()
    }
}

/// What a host lends the core for a stage of the frame.
pub(crate) struct Lent<'a> {
    /// What the frame's input reads as. Filled by the host, cleared by it
    /// after the frame; a subsystem may write it too.
    pub input: &'a mut InputState,
    pub camera: &'a mut Camera,
    /// `None` with no renderer (a test, a run with no GPU).
    pub renderer: Option<&'a mut SceneRenderer>,
    pub gpu: Option<(&'a wgpu::Device, &'a wgpu::Queue)>,
    /// egui's points per pixel, for a pointer in window pixels.
    pub pixels_per_point: f32,
    pub cursor_captured: bool,
    /// Whether the scene file may be written (#364).
    pub scene_file_writable: bool,
    /// What a debug key says when it has nothing to act on (#367).
    pub note: &'a mut super::key_note::KeyNote,
    /// The pads, when the host has them.
    pub pads: Option<super::subsystem::Pads<'a>>,
    /// The run's exit flag, when the host has a run to end.
    pub quit: Option<&'a mut bool>,
}

/// What [`GameCore::player_view`] reads when no view is in the resources.
static NO_VIEW: PlayerView = PlayerView::NONE;

/// The presentation's part in a frame: what [`step`] calls back. Every hook
/// but [`FrameHost::lend`] has a default that does nothing, which is what a
/// host with no window does there.
pub(crate) trait FrameHost {
    /// The core, and what the host lends it for a stage.
    fn lend(&mut self) -> (&mut GameCore, Lent<'_>);

    /// The core alone.
    fn core(&mut self) -> &mut GameCore;

    /// After [`Stage::Input`], before the frame reads the game state or
    /// its events: a scene transition moves on, and extensions'
    /// `pre_update` runs.
    fn before_events(&mut self) {}

    /// The game's extensions at `stage`, after the subsystems (#754). Only
    /// the player has extensions; a capture has none.
    fn extensions_at(&mut self, _stage: Stage) {}

    /// Whether one of the game's extensions took `command` (#754).
    fn extension_takes(&mut self, _command: &ScriptCommand) -> bool {
        false
    }

    /// A phase of the frame is done (the perf readout).
    fn phase_done(&mut self, _phase: Phase) {}

    /// What the scripts read of the window, set on the core's scripts
    /// before their `on_update`: the transition's phase, the screen's size,
    /// the pointer, the cursor, the chunks loaded, the sounds' clocks and
    /// the music session's conducted parameters (the music subsystem sets
    /// those at `Stage::PreScripts`).
    fn window_for_scripts(&mut self) {}

    /// Whether this host is a capture (`flint render`): it steps no
    /// physics, and steps its own animation and particles on fixed steps
    /// (`bones_posed` is where its animation goes), and nothing goes to
    /// stdout, which may be a report.
    fn capture(&self) -> bool {
        false
    }

    /// A script command this host won't do, before any subsystem or the
    /// core sees it (a capture's [`PRESENTATION_ONLY`] list). The host says
    /// why, if it does.
    ///
    /// [`PRESENTATION_ONLY`]: super::headless_game::PRESENTATION_ONLY
    fn refuses(&mut self, _command: &ScriptCommand) -> bool {
        false
    }

    /// A command that is the presentation's ([`for_the_host`]): sound, the
    /// pads, a look, quality, a scene or chunk, quitting. The default
    /// warns once that this host doesn't do it.
    fn host_command(&mut self, command: ScriptCommand) {
        struct Nobody;
        impl ScriptHost for Nobody {}
        flint_script::host::apply(&mut Nobody, command);
    }

    /// `on_draw_ui` has run and its commands are done: what it drew (the
    /// HUD's draw lists, world lines and world text) is the host's.
    fn script_ui_drawn(&mut self) {}

    /// The scripts are done with the frame's world: the models of the
    /// entities spawned since the last frame (a script's `spawn_entity`)
    /// load, before animation and the renderer's sync see them (#1107).
    fn late_models(&mut self) {}

    /// Animation has posed the skeletons: their bones to the GPU.
    fn bones_posed(&mut self) {}

    /// After the procgen stage: the renderer's sync with the world, the
    /// scripts' post-processing for the frame and their quality settings.
    fn sync(&mut self) {}
}

/// One frame of the game, in the order of ADR 0117's stages, with `host`'s
/// presentation where it falls. The clock has ticked and the frame's input
/// is in; the host clears the input after.
pub(crate) fn step<H: FrameHost + ?Sized>(host: &mut H) {
    // Outside input for scripts, queued for their update below (the
    // expression feed, Flint#518)
    run_stage(host, Stage::Input, None);

    // A transition, extensions
    host.before_events();

    // Which systems the game state lets run this frame
    let config = host.core().state_machine.active_config().clone();
    let config = &config;
    let game_events = {
        let (core, lent) = host.lend();
        // Physics events from the previous frame's fixed steps + this
        // frame's input events — scripts and audio both consume them.
        // Drained before the scripts run, so a collision reaches scripts
        // one frame after the step that produced it.
        core.collect_game_events(lent.input)
    };
    host.phase_done(Phase::Input);
    run_stage(host, Stage::PreScripts, Some(config));

    // on_update runs BEFORE the fixed physics loop. Scripts pose kinematic
    // bodies with this frame's delta_time, and PhysicsSystem::begin_frame
    // spreads that motion over this frame's accumulator: the two must
    // describe the same span of wall time. With physics first, the loop
    // spread a pose written with the previous frame's delta over the
    // current frame's accumulator, and whenever consecutive frame times
    // disagreed (a maximized, GPU-bound window) the kinematic parent's
    // step velocity swung by their ratio and jointed children rang.
    host.window_for_scripts();
    {
        let (core, lent) = host.lend();
        // The state guard sets raw state-machine / store / resources
        // pointers for script access and clears them on drop. The physics
        // steps below borrow the resources mutably, so scripts get their
        // own scope here and the later script calls (UI, sprite-anim-end
        // callbacks) another below.
        let _state_scope =
            core.script
                .state_scope(&mut core.state_machine, &mut core.store, &core.resources);
        core.script
            .set_current_scene(&core.scene_path, &core.project);
        core.run_scripts(config, &game_events, lent);
    }
    host.phase_done(Phase::Scripts);
    run_stage(host, Stage::Scripts, Some(config));

    // Fixed-timestep physics and the first-person eye, against the poses
    // and joint targets the scripts just wrote
    let steps_physics = !host.capture();
    // The physics package's frame share (its triangles, `begin_frame`, the
    // character's look) is at `Stage::Scripts`, just above (#1617)
    {
        let (core, lent) = host.lend();
        core.tick_fixed_steps(config, steps_physics, lent.input);
    }
    // The eye a controller placed (ADR 0138's `View`): the first-person
    // package's walker (#1618), a vehicle's chase camera. A capture
    // steps no physics, and its camera stays the scene's.
    host.core()
        .resources
        .get_or_insert_with(PlayerView::default)
        .new_frame();
    run_stage(host, Stage::View, Some(config));
    {
        let (core, lent) = host.lend();
        // The listener's move is `PlayerView::listener`, which the audio
        // package reads at `Stage::Physics` (#1619)
        core.follow_eye(lent.camera);
    }
    host.phase_done(Phase::Physics);
    run_stage(host, Stage::Physics, Some(config));

    // Script pointers again for every remaining script call this frame —
    // including the sprite-anim-end callbacks in the animation stage.
    let _state_scope = {
        let core = host.core();
        core.script
            .state_scope(&mut core.state_machine, &mut core.store, &core.resources)
    };

    // Style tweens (#370) move on with game time, and pause with the
    // scripts that started them. Stepped before this frame's commands, so
    // a tween asked for now starts moving next frame, and a
    // set_style_param this frame overrides (and stops) it.
    host.core().step_style_tweens();
    // The field tweens (#734), the animation package's at its stage
    // (#1620), between the entity styles' and the whole frame's
    run_stage(host, Stage::Tweens, Some(config));
    // And whole-frame styles' (#406), on the renderer's config
    {
        let (core, lent) = host.lend();
        core.step_post_style_tweens(lent.renderer);
    }

    // What on_update asked (#697): the one dispatcher, so the camera a
    // script set is in place for on_draw_ui and the frame
    dispatch(host);

    // Script camera overrides (chase cam etc.) + roll basis + listener.
    // The camera is final for this frame then: on_draw_ui gets the one the
    // frame renders with, so world_to_screen / screen_to_ground there line
    // up with the drawn world instead of trailing it by a frame.
    {
        let (core, lent) = host.lend();
        // The listener moves to it at `Stage::PostPhysics` (the audio
        // package), where no controller placed an eye
        core.apply_script_camera(lent.camera);
    }
    run_stage(host, Stage::PostPhysics, Some(config));

    // on_draw_ui (always runs: pause menus and transition visuals need to
    // draw), then what it asked, then what it drew
    {
        let core = host.core();
        flint_core::scope!("call_draw_uis");
        core.script.call_draw_uis(&mut core.world);
    }
    dispatch(host);
    host.script_ui_drawn();
    host.phase_done(Phase::ScriptUi);
    run_stage(host, Stage::ScriptUi, Some(config));
    host.late_models();

    // Sound triggers (the audio package's `Stage::Sound`, #1619), the
    // animation clock and its ends and cues to the scripts
    // (`Stage::Animate`, the animation package's, #1620), the clock poses
    // are held on, bone matrices, bone probes and IK (`Stage::Pose`), then
    // particles (the particles package's, at `Stage::Animation`, #1622). A
    // capture keeps its own animation, on a fixed step, in `bones_posed`,
    // and runs neither of the animation stages
    let capture = host.capture();
    run_stage_on(host, Stage::Sound, config, &game_events);
    if !capture {
        run_stage(host, Stage::Animate, Some(config));
    }
    host.bones_posed();
    if !capture {
        run_stage(host, Stage::Pose, Some(config));
    }
    // The particles package's step (#1622), timed with the animation as it
    // was when the core stepped them
    run_stage(host, Stage::Animation, Some(config));
    host.phase_done(Phase::AnimFx);

    // The asset makers' frame (procgen's budgeted queue); living props move
    // on after it, in what it left of the budget
    {
        let (core, lent) = host.lend();
        core.tick_asset_makers(lent);
    }
    run_stage(host, Stage::MadeAssets, Some(config));

    // The renderer's sync, the frame's post overrides, quality
    host.sync();
    run_stage(host, Stage::Sync, Some(config));
}

/// Run `stage` on every subsystem. Nothing to do, and nothing lent, when
/// there are none.
pub(crate) fn run_stage<H: FrameHost + ?Sized>(
    host: &mut H,
    stage: Stage,
    policy: Option<&StateConfig>,
) {
    run_stage_with(host, stage, policy, &[]);
}

/// [`run_stage`] with the frame's game events lent (`StageCtx::events`).
fn run_stage_on<H: FrameHost + ?Sized>(
    host: &mut H,
    stage: Stage,
    policy: &StateConfig,
    events: &[GameEvent],
) {
    run_stage_with(host, stage, Some(policy), events);
}

fn run_stage_with<H: FrameHost + ?Sized>(
    host: &mut H,
    stage: Stage,
    policy: Option<&StateConfig>,
    events: &[GameEvent],
) {
    if !host.core().subsystems.is_empty() {
        flint_core::scope!("subsystems", stage.label());
        let (core, lent) = host.lend();
        core.with_subsystems_on(lent, policy, events, |subsystems, ctx| {
            subsystems.update(stage, ctx)
        });
    }
    // Then the game's extensions (ADR 0117 decision 5, #754)
    host.extensions_at(stage);
}

impl GameCore {
    /// `f` with the subsystems and a [`StageCtx`] over the core's fields
    /// and what the host lent, borrowed one by one.
    pub(crate) fn with_subsystems<R>(
        &mut self,
        lent: Lent<'_>,
        policy: Option<&StateConfig>,
        f: impl FnOnce(&mut Subsystems, &mut StageCtx) -> R,
    ) -> R {
        self.with_subsystems_on(lent, policy, &[], f)
    }

    /// [`with_subsystems`](Self::with_subsystems), the frame's `events`
    /// lent too (audio's triggers at `Stage::Sound`).
    pub(crate) fn with_subsystems_on<R>(
        &mut self,
        lent: Lent<'_>,
        policy: Option<&StateConfig>,
        events: &[GameEvent],
        f: impl FnOnce(&mut Subsystems, &mut StageCtx) -> R,
    ) -> R {
        // The sound is the run's resource (#1619), lent with the rest: the
        // Package API names no standard package's type
        let Lent {
            input,
            camera,
            renderer,
            gpu,
            pixels_per_point,
            cursor_captured,
            scene_file_writable,
            note,
            pads,
            quit,
        } = lent;
        {
            let mut ctx = StageCtx {
                world: &mut self.world,
                clock: &self.clock,
                input,
                script: &mut self.script,
                camera,
                policy,
                renderer,
                gpu,
                project: &self.project,
                scene_path: &self.scene_path,
                scene_file_writable,
                pixels_per_point,
                cursor_captured,
                note,
                requests: &mut self.script_requests,
                pads,
                quit,
                resources: &mut self.resources,
                events,
                bus: &mut self.events,
            };
            f(&mut self.subsystems, &mut ctx)
        }
    }

    /// The bus's events (the last frame's physics contacts, extension,
    /// style-tween and script events), then this frame's actions pressed and
    /// released. Always collected: input is processed while paused, so
    /// pause/unpause keybinds work.
    fn collect_game_events(&mut self, input: &InputState) -> Vec<GameEvent> {
        flint_core::scope!("collect_game_events");
        let mut game_events = self.events.drain();
        if let Some(log) = &mut self.event_log {
            for event in &game_events {
                if let GameEvent::Custom { name, .. } = event {
                    log.push((self.clock.total_time, name.clone()));
                }
            }
        }
        for action in input.actions_just_pressed() {
            game_events.push(GameEvent::ActionPressed(action));
        }
        for action in input.actions_just_released() {
            game_events.push(GameEvent::ActionReleased(action));
        }
        game_events
    }

    /// Hand scripts' style reads (#372) the settings the renderer draws
    /// with, copied only when they've changed since the last hand-over.
    fn publish_style_reader(&mut self, renderer: Option<&SceneRenderer>) {
        flint_core::scope!("publish_style_reader");
        let Some(renderer) = renderer else {
            return;
        };
        let generation = renderer.post_config_generation();
        if self.style_reader_generation != Some(generation) {
            self.style_reader_generation = Some(generation);
            let config = renderer.effective_post_process_config().clone();
            self.script.set_style_reader(std::sync::Arc::new(
                super::style_commands::SceneStyles::new(config, renderer.styles().clone()),
            ));
        }
    }

    /// The scripts' context for the frame (the style reader, the camera,
    /// input and events, the clock), then `on_update` unless the game state
    /// pauses scripts.
    fn run_scripts(&mut self, config: &StateConfig, events: &[GameEvent], lent: Lent<'_>) {
        flint_core::scope!("run_scripts");
        self.publish_style_reader(lent.renderer.as_deref());
        // Who `on_interact` and `find_nearest_interactable` measure from
        self.script.set_player(self.player_view().player);
        let camera = &*lent.camera;
        self.script
            .set_camera(camera.position_array(), camera.forward_vector());
        self.script
            .set_camera_view_proj(camera.view_projection_matrix());
        self.script.provide_context(
            lent.input,
            events,
            self.clock.total_time,
            self.clock.delta_time,
        );
        if config.runs(SCRIPTS) {
            self.script
                .update(&mut self.world, self.clock.delta_time)
                .unwrap_or_else(|e| tracing::warn!("Script error: {:?}", e));
        }
    }

    /// The fixed-timestep loop (ADR 0138 decision 2, #1617): each step that
    /// steps walks the step stages (`StepStage::ALL`: the walk, physics'
    /// sync-in, every subsystem's forces, Rapier's step) over the
    /// subsystems. Skipped while the game state pauses physics, and in a
    /// capture, but the steps are still consumed, to avoid a spiral.
    fn tick_fixed_steps(&mut self, config: &StateConfig, steps: bool, input: &InputState) {
        flint_core::scope!("tick_fixed_physics");
        self.fixed_steps = 0;
        // The steps this frame runs, for a subsystem's `FixedCtx::step`
        let frame_steps = self.clock.pending_fixed_steps() as u32;
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            if steps && config.runs(PHYSICS) {
                let step = self.fixed_steps;
                self.fixed_steps += 1;
                let mut ctx = FixedCtx {
                    world: &mut self.world,
                    dt,
                    step: (step, frame_steps.max(step + 1)),
                    input,
                    events: &mut self.events,
                    policy: config,
                    resources: &mut self.resources,
                };
                for stage in flint_package::StepStage::ALL {
                    flint_core::scope!("subsystems", stage.label());
                    self.subsystems.step(stage, &mut ctx);
                }
            }

            self.clock.consume_fixed_step();
        }
    }

    /// The camera to the frame's eye, after `Stage::View` (ADR 0138
    /// decision 2): whichever controller placed it, physics' first-person
    /// eye or a package's camera (the vehicle's chase camera, #1631).
    /// Returns where the listener goes (`PlayerView::listener`, which the
    /// audio package reads), when an eye was placed; with none the camera stays where the scene and the scripts
    /// put it.
    fn follow_eye(&mut self, camera: &mut Camera) -> Option<Eye> {
        let view = self.resources.get_or_insert_with(PlayerView::default);
        let eye = view.eye?;
        camera.update_first_person(eye.position, eye.yaw, eye.pitch);
        camera.target = eye.target;
        if let Some(fov) = eye.fov {
            camera.fov = fov;
        }
        // The audio package moves its listener there at `Stage::Physics`,
        // when its policy runs (#1619)
        view.listener = Some(eye);
        view.listener
    }

    /// Who the game is played as, and where it is seen from (ADR 0138
    /// decision 4, #1614): a run resource that a controller writes, read by
    /// the cursor, the controls line, the listener and the scripts.
    pub fn player_view(&self) -> &PlayerView {
        self.resources.get::<PlayerView>().unwrap_or(&NO_VIEW)
    }

    /// [`player_view`](Self::player_view), to write.
    pub(crate) fn player_view_mut(&mut self) -> &mut PlayerView {
        self.resources.get_or_insert_with(PlayerView::default)
    }

    /// The scene's view at its entry, before its builders: reset, so a
    /// controller's builder (the first-person package's, #1618) says
    /// who its player is before the first frame (#1614's risk: the cursor
    /// and the controls line read it at the end of the entry), and a scene
    /// with none has no player.
    pub(crate) fn enter_player_view(&mut self) {
        self.resources.insert(PlayerView::default());
        self.script.set_player(None);
    }

    /// The view forgotten with its scene, at a teardown.
    pub(crate) fn forget_player_view(&mut self) {
        *self.player_view_mut() = PlayerView::default();
        self.script.set_player(None);
    }

    /// Move the style tweens (#370) on by this frame's game time, while
    /// scripts run (the field tweens are the animation package's, at
    /// `Stage::Tweens` right after, #1620).
    fn step_style_tweens(&mut self) {
        if self.state_machine.active_config().runs(SCRIPTS) {
            flint_core::scope!("step_style_tweens");
            let dt = self.clock.delta_time;
            let step = self.style_tweens.step(&mut self.world, &self.styles, dt);
            for event in step.events {
                self.events.push(event);
            }
            for message in step.failures {
                tracing::warn!(target: "script", "{message}");
            }
        }
    }

    /// Move the whole-frame style tweens on by this frame's game time
    /// (#406), while scripts run, and hand the renderer its config back
    /// when one moved. Without a renderer there is no frame to style.
    fn step_post_style_tweens(&mut self, renderer: Option<&mut SceneRenderer>) {
        if !self.state_machine.active_config().runs(SCRIPTS) {
            return;
        }
        let Some(renderer) = renderer else {
            return;
        };
        if self.style_tweens.post_len() == 0 {
            return;
        }
        let mut config = renderer.post_process_config().clone();
        let (step, moved) =
            self.style_tweens
                .step_post(&mut config, &self.styles, self.clock.delta_time);
        if moved {
            renderer.set_post_process_config(config);
        }
        for event in step.events {
            self.events.push(event);
        }
        for message in step.failures {
            tracing::warn!(target: "script", "{message}");
        }
    }

    /// The camera where the scripts asked this frame, and the scripts told
    /// where it is now. Returns the listener's move for a script-driven
    /// camera (chase cam etc.) that set a position.
    fn apply_script_camera(&mut self, camera: &mut Camera) -> Option<Eye> {
        let mut cam = std::mem::take(&mut self.script_requests.camera);
        // An eye's roll (a chase camera's lean, #1631) where the scripts
        // set none this frame: the overrides level the camera otherwise
        if cam.roll.is_none() {
            cam.roll = self
                .player_view()
                .eye
                .map(|eye| eye.roll)
                .filter(|roll| *roll != 0.0);
        }
        super::script_commands::apply_camera_overrides(camera, &cam);
        self.script
            .set_camera(camera.position_array(), camera.forward_vector());
        self.script
            .set_camera_view_proj(camera.view_projection_matrix());
        // The listener follows a script's camera only where no controller
        // placed an eye (ADR 0138)
        let view = self.resources.get_or_insert_with(PlayerView::default);
        if view.eye.is_some() || cam.position.is_none() {
            return None;
        }
        let eye = Eye::looking(camera.position, camera.target);
        view.listener = Some(eye);
        Some(eye)
    }

    /// The scene's made assets, before the model loader (#32, #1557): each
    /// asset maker the run registered makes the names it can (the procgen
    /// package's specs, living's props through its load step), the rest
    /// staying the file loader's. A run with no maker says (with
    /// `say_unmade`) what a spec would have made; so does one whose
    /// makers' packages the project's `[packages]` leaves out (#1578), in
    /// `[packages]`' words.
    pub(crate) fn make_scene_assets(
        &mut self,
        gpu: flint_package::assets::Gpu<'_>,
        tables: &toml::Table,
        config: &flint_render::model_loader::ModelLoadConfig,
        mode: flint_package::assets::MakeMode,
        say_unmade: bool,
    ) {
        let registry = crate::packages::registry();
        if say_unmade {
            self.say_unmade(registry);
        }
        let makers = self.asset_makers();
        if makers.is_empty() {
            return;
        }
        // Where a made mesh's triangles go, out of the resources while a
        // maker is lent them
        let mut made = self.resources.remove::<MadeTriangles>().unwrap_or_default();
        for maker in makers {
            maker.scene(&mut flint_package::assets::MakeCtx {
                world: &mut self.world,
                scene_path: &self.scene_path,
                tables,
                config,
                gpu: flint_package::assets::Gpu {
                    renderer: &mut *gpu.renderer,
                    device: gpu.device,
                    queue: gpu.queue,
                },
                colliders: &mut made,
                resources: &mut self.resources,
                registry,
                packages: self.packages.as_ref(),
                mode,
            });
        }
        self.resources.insert(made);
    }

    /// Say, once at the scene's load, what the scene's names a spec would
    /// make do when nothing makes them: a build with no asset maker says
    /// so (`procgen_line`), and a project whose `[packages]` leaves out a
    /// maker's package says that, with the line to add (#1578).
    fn say_unmade(&self, registry: &flint_package::Registry) {
        let lines: Vec<String> = if registry.asset_makers().next().is_none() {
            super::missing_features::procgen_line(&self.world, &self.project, &self.scene_path)
                .into_iter()
                .collect()
        } else if let Some(in_use) = &self.packages {
            registry
                .asset_makers_left_out(Some(in_use))
                .into_iter()
                .filter_map(|package| {
                    super::missing_features::left_out_maker_line(
                        in_use,
                        package,
                        &self.world,
                        &self.project,
                        &self.scene_path,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        for line in lines {
            tracing::warn!(target: "load", "{line}");
        }
    }

    /// Each asset maker's frame (#1557): the procgen package's budgeted
    /// queue, whose finished assets go to the renderer when there is one.
    /// Living props move on after it, at `Stage::MadeAssets`, in whatever it
    /// left of the budget.
    pub(crate) fn tick_asset_makers(&mut self, lent: Lent<'_>) {
        let makers = self.asset_makers();
        if makers.is_empty() {
            return;
        }
        let mut gpu = match (lent.renderer, lent.gpu) {
            (Some(renderer), Some((device, queue))) => Some(flint_package::assets::Gpu {
                renderer,
                device,
                queue,
            }),
            _ => None,
        };
        let mut made = self.resources.remove::<MadeTriangles>().unwrap_or_default();
        for maker in makers {
            maker.frame(&mut flint_package::assets::FrameMake {
                resources: &mut self.resources,
                camera: lent.camera.position_array(),
                project: &self.project,
                scene_path: &self.scene_path,
                gpu: gpu.as_mut().map(|g| flint_package::assets::Gpu {
                    renderer: &mut *g.renderer,
                    device: g.device,
                    queue: g.queue,
                }),
                colliders: &mut made,
            });
        }
        self.resources.insert(made);
    }
}

/// Carry out what the scripts have asked since the last drain (#697), in
/// the order they asked: what the host refuses is dropped first, then each
/// subsystem takes its own (living props, the wooze field), then each of the
/// rest goes through the one dispatcher ([`flint_script::host::apply`]) to
/// the core, or, if it's the presentation's ([`for_the_host`]), to the
/// host. The frame drains twice: after the fixed physics steps, for what
/// `on_update` (and last frame's late callbacks) asked, so a camera a
/// script set is the one `on_draw_ui` sees and the frame draws; and after
/// `on_draw_ui`, for what it asked.
pub(crate) fn dispatch<H: FrameHost + ?Sized>(host: &mut H) {
    let commands = host.core().script.drain_commands();
    if !commands.is_empty() {
        dispatch_commands(host, commands);
    }
}

/// [`dispatch`] of `commands`, as if the scripts had just asked for them.
pub(crate) fn dispatch_commands<H: FrameHost + ?Sized>(
    host: &mut H,
    mut commands: Vec<ScriptCommand>,
) {
    flint_core::scope!("dispatch_commands", commands.len());
    commands.retain(|command| !host.refuses(command));
    if !host.core().subsystems.is_empty() && !commands.is_empty() {
        let (core, lent) = host.lend();
        core.with_subsystems(lent, None, |subsystems, ctx| {
            commands.retain(|command| !subsystems.script_command(command, ctx));
        });
    }
    // Then the game's extensions, for their modules' commands (#754)
    commands.retain(|command| !host.extension_takes(command));
    let quiet = host.capture();
    for command in commands {
        if for_the_host(&command) {
            host.host_command(command);
            continue;
        }
        let (core, lent) = host.lend();
        let mut commands = CoreCommands {
            core,
            renderer: lent.renderer,
            quiet,
        };
        flint_script::host::apply(&mut commands, command);
    }
}

/// Whether `command` is the presentation's, which the host carries out:
/// sound and the pads, the camera-cut detector, a look (it re-reads the
/// scene's post-processing), quality (the renderer's levers), a style's
/// reset to what the scene authored, another scene or a chunk, quitting,
/// and the end of a transition. Everything else changes the game and the
/// core does it, the same for every host.
pub(crate) fn for_the_host(command: &ScriptCommand) -> bool {
    use ScriptCommand as C;
    match command {
        C::FireEvent { name, .. } => name == flint_core::events::TRANSITION_COMPLETE,
        C::PlaySound { .. }
        | C::PlaySoundAt { .. }
        | C::StopSound { .. }
        | C::SetSoundVolume { .. }
        | C::SetBusVolume { .. }
        | C::SetUserBusVolume { .. }
        | C::SetBusLowpass { .. }
        | C::Rumble { .. }
        | C::SetCameraCutDetection { .. }
        | C::SetLook { .. }
        | C::PrepareLook { .. }
        | C::SetQuality { .. }
        | C::ResetPostStyle { .. }
        | C::LoadScene { .. }
        | C::ReloadScene
        | C::QuitGame
        | C::LoadChunk { .. }
        | C::UnloadChunk { .. } => true,
        _ => false,
    }
}

/// Hand a style command's outcome on: its event to scripts' `on_event`
/// (next frame, with the physics events), its refusal to the log.
fn style_applied(core: &mut GameCore, applied: StyleApplied) {
    match applied {
        StyleApplied::Done(Some(event)) => core.events.push(event),
        StyleApplied::Done(None) => {}
        StyleApplied::Failed(message) => tracing::warn!(target: "script", "{message}"),
        // Applied, past its range (#492)
        StyleApplied::Clamped(event, message) => {
            tracing::warn!(target: "script", "{message}");
            if let Some(event) = event {
                core.events.push(event);
            }
        }
    }
}

/// A whole-frame style command (#369): edit the renderer's config and hand
/// it back; the next frame builds a style newly switched on. Without a
/// renderer there is no frame to style.
fn edit_post_styles(
    core: &mut GameCore,
    renderer: Option<&mut SceneRenderer>,
    edit: impl FnOnce(
        &mut flint_render::PostProcessConfig,
        &flint_render::stylize::StyleRegistry,
    ) -> StyleApplied,
) {
    let Some(renderer) = renderer else {
        return;
    };
    let mut config = renderer.post_process_config().clone();
    let applied = edit(&mut config, &core.styles);
    if applied.applied() {
        renderer.set_post_process_config(config);
    }
    style_applied(core, applied);
}

/// A script's `reset_post_style`: the style as `authored`, the scene's own
/// settings, which only the host knows (a host command, [`for_the_host`]).
pub(crate) fn reset_post_style(
    core: &mut GameCore,
    renderer: Option<&mut SceneRenderer>,
    authored: &flint_render::PostProcessConfig,
    style: &str,
) {
    // The scene's values win over a tween's (#406)
    core.style_tweens.stop_post(style, None);
    edit_post_styles(core, renderer, |config, styles| {
        super::style_commands::reset_post_style(config, styles, authored, style)
    });
}

/// The core as a script host: every command that isn't the presentation's,
/// carried out on the core's state, and on the renderer when the host lent
/// one (the whole-frame styles and their history).
struct CoreCommands<'a> {
    core: &'a mut GameCore,
    renderer: Option<&'a mut SceneRenderer>,
    /// A capture's stdout may be a report: state changes go to the log.
    quiet: bool,
}

impl CoreCommands<'_> {
    /// Hand a style command's outcome on: its event to scripts' `on_event`
    /// (next frame, with the physics events), its refusal to the log.
    fn style_applied(&mut self, applied: StyleApplied) {
        style_applied(self.core, applied);
    }

    /// A script's panel command (#880): the renderer's layout edited, or a
    /// warning naming the function when the edit doesn't parse. Without a
    /// renderer there is no page.
    fn edit_panels(
        &mut self,
        function: &str,
        edit: impl FnOnce(
            Option<&flint_render::panels::PanelLayout>,
        ) -> Result<Option<flint_render::panels::PanelLayout>, String>,
    ) {
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return;
        };
        match edit(renderer.panel_layout()) {
            Ok(layout) => renderer.set_panel_layout(layout),
            Err(e) => tracing::warn!(target: "script", "{function}: [panels] {e}"),
        }
    }

    /// A whole-frame style command (#369): edit the renderer's config and
    /// hand it back; the next frame builds a style newly switched on.
    /// Without a renderer there is no frame to style.
    fn edit_post_styles(
        &mut self,
        edit: impl FnOnce(
            &mut flint_render::PostProcessConfig,
            &flint_render::stylize::StyleRegistry,
        ) -> StyleApplied,
    ) {
        edit_post_styles(self.core, self.renderer.as_deref_mut(), edit);
    }

    /// A module's command no subsystem took (#698): its subsystem went
    /// with its scene after the call (a command queued in `on_scene_exit`).
    /// A call where the module isn't enabled never queues one. Said once a
    /// run, since a script may ask every frame.
    fn module_gone(&mut self, function: &'static str) {
        if self.core.commands_warned.insert(function) {
            tracing::warn!(
                target: "script",
                "{function}: the subsystem its module belongs to is gone with its scene; dropped"
            );
        }
    }

    /// A game state change, said where the player says it.
    fn said(&self, line: String) {
        if self.quiet {
            tracing::info!("{line}");
        } else {
            tracing::info!(target: "script", "{line}");
        }
    }
}

impl ScriptHost for CoreCommands<'_> {
    fn host_name(&self) -> &'static str {
        "the game"
    }

    fn set_audio_lowpass(&mut self, cutoff_hz: f32) {
        self.core.script_requests.audio_lowpass = Some(cutoff_hz);
    }

    fn fire_event(&mut self, name: String, data: toml::Value) {
        self.core.events.push(GameEvent::Custom { name, data });
    }

    fn set_entity_style(&mut self, entity: EntityId, style: String) {
        let core = &mut *self.core;
        let applied =
            core.style_tweens
                .set_entity_style(&mut core.world, &core.styles, entity, &style);
        self.style_applied(applied);
    }

    fn set_style_param(&mut self, entity: EntityId, param: String, value: toml::Value) {
        let core = &mut *self.core;
        let applied =
            core.style_tweens
                .set_style_param(&mut core.world, &core.styles, entity, &param, value);
        self.style_applied(applied);
    }

    fn tween_style_param(
        &mut self,
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        easing: String,
    ) {
        // A tween starts from the scene's style table where an entity has
        // no value of its own (#370)
        let applied = {
            let fallback;
            let config = match self.renderer.as_deref() {
                Some(r) => r.post_process_config(),
                None => {
                    fallback = flint_render::PostProcessConfig::default();
                    &fallback
                }
            };
            let core = &mut *self.core;
            core.style_tweens.tween(
                &mut core.world,
                &core.styles,
                config,
                entity,
                &param,
                &target,
                seconds,
                &easing,
            )
        };
        self.style_applied(applied);
    }

    fn clear_entity_style(&mut self, entity: EntityId) {
        let core = &mut *self.core;
        let applied = core
            .style_tweens
            .clear_entity_style(&mut core.world, &core.styles, entity);
        self.style_applied(applied);
    }

    fn set_post_style(&mut self, style: String, enabled: bool) {
        self.edit_post_styles(|config, styles| {
            super::style_commands::set_post_style(config, styles, &style, enabled)
        });
    }

    fn set_post_style_param(&mut self, style: String, param: String, value: toml::Value) {
        // The script took over from a tween of it (#406)
        self.core.style_tweens.stop_post(&style, Some(&param));
        self.edit_post_styles(|config, styles| {
            super::style_commands::set_post_style_param(config, styles, &style, &param, value)
        });
    }

    fn tween_post_style_param(
        &mut self,
        style: String,
        param: String,
        target: f64,
        seconds: f64,
        easing: String,
    ) {
        let core = &mut *self.core;
        let tweens = &mut core.style_tweens;
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return;
        };
        let mut config = renderer.post_process_config().clone();
        let applied = tweens.tween_post(
            &mut config,
            &core.styles,
            &style,
            &param,
            target,
            seconds,
            &easing,
        );
        if applied.applied() {
            renderer.set_post_process_config(config);
        }
        style_applied(core, applied);
    }

    fn reset_style_history(&mut self) {
        if let Some(renderer) = self.renderer.as_deref_mut() {
            renderer.reset_style_history();
        }
    }

    fn set_panels(&mut self, layout: toml::Table) {
        self.edit_panels("set_panels", |_| {
            flint_render::panels::PanelLayout::parse(&layout)
        });
    }

    fn set_panel(&mut self, name: String, keys: toml::Table) {
        self.edit_panels("set_panel", |layout| match layout {
            Some(layout) => layout.with_panel(&name, &keys).map(Some),
            None => {
                let mut table = toml::Table::new();
                flint_render::panels::merge_panel(&mut table, &name, &keys);
                flint_render::panels::PanelLayout::parse(&table)
            }
        });
    }

    fn remove_panel(&mut self, name: String) {
        self.edit_panels("remove_panel", |layout| match layout {
            Some(layout) => layout.without_panel(&name),
            None => Ok(None),
        });
    }

    fn clear_panels(&mut self) {
        self.edit_panels("clear_panels", |_| Ok(None));
    }

    fn set_camera(&mut self, camera: CameraOverrides) {
        self.core.script_requests.camera.merge(camera);
    }

    fn set_post_process(&mut self, post: PostProcessOverrides) {
        self.core.script_requests.post.merge(post);
    }

    fn set_render_mode(&mut self, mode: Option<(u32, f32)>, params: Option<[f32; 4]>) {
        let r = &mut self.core.script_requests;
        r.render_mode = mode.or(r.render_mode);
        r.mode_params = params.or(r.mode_params);
    }

    fn set_lod_scales(&mut self, lod_scale: Option<f32>, draw_distance_scale: Option<f32>) {
        let r = &mut self.core.script_requests;
        r.lod_scale = lod_scale.or(r.lod_scale);
        r.draw_distance_scale = draw_distance_scale.or(r.draw_distance_scale);
    }

    fn set_time_scale(&mut self, scale: f64) {
        // This frame's clock has ticked, so the new scale starts with the
        // next frame's
        self.core.clock.time_scale = scale;
    }

    fn set_touch_controls(&mut self, enabled: bool) {
        self.core.script_requests.touch_controls = Some(enabled);
    }

    fn set_cursor_captured(&mut self, captured: bool) {
        self.core.script_requests.cursor_captured = Some(captured);
    }

    // A subsystem takes its module's commands (`LivingSubsystem::command`)
    // before they reach here.
    fn module_command(&mut self, command: flint_script::ModuleCommand) {
        self.module_gone(command.function());
    }

    fn push_state(&mut self, name: String) {
        if self.core.state_machine.push_state(&name) {
            self.said(format!("[state] Pushed '{name}'"));
        } else {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }

    fn pop_state(&mut self) {
        if let Some(popped) = self.core.state_machine.pop_state() {
            self.said(format!("[state] Popped '{}'", popped.name));
        }
    }

    fn replace_state(&mut self, name: String) {
        if self.core.state_machine.replace_state(&name) {
            self.said(format!("[state] Replaced top with '{name}'"));
        } else {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }
}
