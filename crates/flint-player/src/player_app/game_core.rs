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
use super::subsystem::{Stage, StageCtx, Subsystems};
use flint_animation::AnimationSystem;
use flint_core::project::Project;
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_procgen::ProcGenResolver;
use flint_render::{Camera, SceneRenderer};
use flint_runtime::state::{ANIMATION, AUDIO, PARTICLES, PHYSICS, SCRIPTS};
use flint_runtime::{
    EventBus, GameClock, GameEvent, GameStateMachine, InputState, PersistentStore, RuntimeSystem,
    StateConfig,
};
use flint_script::ScriptSystem;

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
    pub physics: PhysicsSystem,
    /// The frame's game events (#727): physics pushes its contacts at each
    /// fixed step, extensions, style tweens and scripts' `fire_event` their
    /// own, and the frame drains them before scripts run. Physics' `clear`
    /// at a transition leaves it alone.
    pub(crate) events: EventBus,
    pub animation: AnimationSystem,
    pub particles: ParticleSystem,
    pub script: ScriptSystem,
    /// Game states and the persistent store survive scene transitions. The
    /// machine knows the built-in systems and every scene subsystem's
    /// policy name (`subsystem::state_machine`, #727).
    pub(crate) state_machine: GameStateMachine,
    pub(crate) store: PersistentStore,
    /// Entity style tweens (#370), stepped with game time.
    pub(crate) style_tweens: super::style_commands::StyleTweens,
    /// The renderer's config generation the scripts' style reader was
    /// last made from (#372).
    pub(crate) style_reader_generation: Option<u64>,
    /// The procgen resolver, which holds the scene's living props.
    pub(crate) procgen_resolver: ProcGenResolver,
    /// What scripts asked of the frame (#697): the camera, post-processing,
    /// distance levers, cursor and touch, held from the command that asked
    /// to the step of the frame that applies each.
    pub(crate) script_requests: flint_script::FrameRequests,
}

impl GameCore {
    /// A core for `world`, loaded from `scene_path`, with nothing started.
    pub fn new(world: FlintWorld, scene_path: String, project: Project) -> Self {
        Self {
            subsystems: Default::default(),
            world,
            scene_path,
            project,
            clock: GameClock::new(),
            physics: PhysicsSystem::new(),
            events: EventBus::new(),
            animation: AnimationSystem::new(),
            particles: ParticleSystem::new(),
            script: ScriptSystem::new(),
            state_machine: super::subsystem::state_machine(),
            store: PersistentStore::new(),
            style_tweens: Default::default(),
            style_reader_generation: None,
            procgen_resolver: ProcGenResolver::new(),
            script_requests: Default::default(),
        }
    }
}

/// What a host lends the core for a stage of the frame.
pub(crate) struct Lent<'a> {
    /// What the frame's input reads as. Filled by the host, cleared by it
    /// after the frame.
    pub input: &'a InputState,
    pub camera: &'a mut Camera,
    /// `None` with no renderer (a test, a run with no GPU).
    pub renderer: Option<&'a mut SceneRenderer>,
    pub gpu: Option<(&'a wgpu::Device, &'a wgpu::Queue)>,
    /// egui's points per pixel, for a pointer in window pixels.
    pub pixels_per_point: f32,
    pub cursor_captured: bool,
    /// Whether the scene file may be written (#364).
    pub scene_file_writable: bool,
    #[cfg(feature = "debug-hud")]
    pub panels: &'a mut Vec<Box<dyn flint_debug_ui::DebugPanel>>,
}

/// Where the sound's listener goes, when the camera moved it this frame.
pub(crate) enum Listener {
    /// The first-person camera, after the physics steps: where it is, and
    /// its yaw and pitch.
    FirstPerson(flint_core::Vec3, f32, f32),
    /// A script moved the camera (a chase camera): to where it now looks.
    ScriptCamera,
}

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

    /// A phase of the frame is done (the perf readout).
    fn phase_done(&mut self, _phase: Phase) {}

    /// What the scripts read of the window, set on the core's scripts
    /// before their `on_update`: the transition's phase, the screen's size,
    /// the pointer, the cursor, the chunks loaded, the sounds' clocks and
    /// the music session's conducted parameters.
    fn window_for_scripts(&mut self) {}

    /// The camera moved the sound's listener.
    fn listener(&mut self, _at: Listener) {}

    /// Carry out what the scripts have asked since the last drain.
    fn dispatch_script_commands(&mut self) {}

    /// `on_draw_ui` has run and its commands are done: what it drew (the
    /// HUD's draw lists, world lines and world text) is the host's.
    fn script_ui_drawn(&mut self) {}

    /// The frame's sound, before animation moves on: its triggers from the
    /// game events, and the sources' update.
    fn sound(&mut self, _policy: &StateConfig, _events: &[GameEvent]) {}

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
    let (has_fps_player, game_events) = {
        let (core, lent) = host.lend();
        // Physics events from the previous frame's fixed steps + this
        // frame's input events — scripts and audio both consume them.
        // Drained before the scripts run, so a collision reaches scripts
        // one frame after the step that produced it.
        (
            core.physics.has_player_entity(),
            core.collect_game_events(lent.input),
        )
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
        // The state guard sets raw state-machine / store / physics pointers
        // for script access and clears them on drop. The physics steps
        // below borrow physics mutably, so scripts get their own scope here
        // and the later script calls (UI, sprite-anim-end callbacks)
        // another below.
        let _state_scope =
            core.script
                .state_scope(&mut core.state_machine, &mut core.store, &core.physics);
        core.script
            .set_current_scene(&core.scene_path, &core.project);
        core.run_scripts(config, &game_events, lent);
    }
    host.phase_done(Phase::Scripts);
    run_stage(host, Stage::Scripts, Some(config));

    // Fixed-timestep physics + FPS camera/listener follow, against the
    // poses and joint targets the scripts just wrote
    let moved = {
        let (core, lent) = host.lend();
        core.tick_fixed_physics(config, has_fps_player, lent.input, lent.camera)
    };
    if let Some(at) = moved {
        host.listener(at);
    }
    host.phase_done(Phase::Physics);
    run_stage(host, Stage::Physics, Some(config));

    // Script pointers again for every remaining script call this frame —
    // including the sprite-anim-end callbacks in the animation stage.
    let _state_scope = {
        let core = host.core();
        core.script
            .state_scope(&mut core.state_machine, &mut core.store, &core.physics)
    };

    // Style tweens (#370) move on with game time, and pause with the
    // scripts that started them. Stepped before this frame's commands, so
    // a tween asked for now starts moving next frame, and a
    // set_style_param this frame overrides (and stops) it.
    host.core().step_style_tweens();

    // What on_update asked (#697): the one dispatcher, so the camera a
    // script set is in place for on_draw_ui and the frame
    host.dispatch_script_commands();

    // Script camera overrides (chase cam etc.) + roll basis + listener.
    // The camera is final for this frame then: on_draw_ui gets the one the
    // frame renders with, so world_to_screen / screen_to_ground there line
    // up with the drawn world instead of trailing it by a frame.
    let moved = {
        let (core, lent) = host.lend();
        core.apply_script_camera(has_fps_player, lent.camera)
    };
    if let Some(at) = moved {
        host.listener(at);
    }
    run_stage(host, Stage::PostPhysics, Some(config));

    // on_draw_ui (always runs: pause menus and transition visuals need to
    // draw), then what it asked, then what it drew
    {
        let core = host.core();
        core.script.call_draw_uis(&mut core.world);
    }
    host.dispatch_script_commands();
    host.script_ui_drawn();
    host.phase_done(Phase::ScriptUi);
    run_stage(host, Stage::ScriptUi, Some(config));

    // Sound triggers, animations + sprite-end callbacks, bone matrices,
    // bone probes, IK, particles
    host.sound(config, &game_events);
    host.core().animate(config);
    host.bones_posed();
    host.core().after_animation(config);
    host.phase_done(Phase::AnimFx);
    run_stage(host, Stage::Animation, Some(config));

    // The frame-budgeted procgen queue; living props move on after it, in
    // what it left of the budget
    {
        let (core, lent) = host.lend();
        core.tick_procgen(lent);
    }
    run_stage(host, Stage::Procgen, Some(config));

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
    if host.core().subsystems.is_empty() {
        return;
    }
    let (core, lent) = host.lend();
    core.with_subsystems(lent, policy, |subsystems, ctx| {
        subsystems.update(stage, ctx)
    });
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
        let mut ctx = StageCtx {
            world: &mut self.world,
            clock: &self.clock,
            input: lent.input,
            script: &mut self.script,
            camera: lent.camera,
            policy,
            renderer: lent.renderer,
            gpu: lent.gpu,
            resolver: &mut self.procgen_resolver,
            project: &self.project,
            scene_path: &self.scene_path,
            scene_file_writable: lent.scene_file_writable,
            pixels_per_point: lent.pixels_per_point,
            cursor_captured: lent.cursor_captured,
            #[cfg(feature = "debug-hud")]
            panels: lent.panels,
        };
        f(&mut self.subsystems, &mut ctx)
    }

    /// The bus's events (the last frame's physics contacts, extension,
    /// style-tween and script events), then this frame's actions pressed and
    /// released. Always collected: input is processed while paused, so
    /// pause/unpause keybinds work.
    fn collect_game_events(&mut self, input: &InputState) -> Vec<GameEvent> {
        let mut game_events = self.events.drain();
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
    #[profiling::function]
    fn publish_style_reader(&mut self, renderer: Option<&SceneRenderer>) {
        let Some(renderer) = renderer else {
            return;
        };
        let generation = renderer.post_config_generation();
        if self.style_reader_generation != Some(generation) {
            self.style_reader_generation = Some(generation);
            let config = renderer.effective_post_process_config().clone();
            self.script.set_style_reader(std::sync::Arc::new(
                super::style_commands::SceneStyles::new(config),
            ));
        }
    }

    /// The scripts' context for the frame (the style reader, the camera,
    /// input and events, the clock), then `on_update` unless the game state
    /// pauses scripts.
    fn run_scripts(&mut self, config: &StateConfig, events: &[GameEvent], lent: Lent<'_>) {
        self.publish_style_reader(lent.renderer.as_deref());
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

    /// The fixed-timestep physics loop (skipped while paused, but still
    /// consuming steps to avoid a spiral), then the first-person camera.
    /// Returns where the listener goes, when the sound runs and a
    /// first-person player moved the camera.
    #[profiling::function]
    fn tick_fixed_physics(
        &mut self,
        config: &StateConfig,
        has_fps_player: bool,
        input: &InputState,
        camera: &mut Camera,
    ) -> Option<Listener> {
        self.physics.begin_frame(self.clock.pending_fixed_ratio());
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            if config.runs(PHYSICS) {
                if has_fps_player {
                    self.physics.update_character(input, &mut self.world, dt);
                }
                self.physics
                    .fixed_update(&mut self.world, dt, &mut self.events)
                    .unwrap_or_else(|e| tracing::warn!("Physics error: {:?}", e));
            }

            self.clock.consume_fixed_step();
        }

        // Update camera from player character position (FPS mode)
        if !has_fps_player {
            return None;
        }
        let cam_pos = self.physics.camera_position(&self.world);
        let cam_target = self.physics.camera_target(cam_pos);
        camera.update_first_person(
            cam_pos,
            self.physics.camera_yaw(),
            self.physics.camera_pitch(),
        );
        camera.target = cam_target;
        config.runs(AUDIO).then(|| {
            Listener::FirstPerson(
                cam_pos,
                self.physics.camera_yaw(),
                self.physics.camera_pitch(),
            )
        })
    }

    /// Move the style tweens on by this frame's game time (#370), while
    /// scripts run.
    fn step_style_tweens(&mut self) {
        if self.state_machine.active_config().runs(SCRIPTS) {
            let step = self
                .style_tweens
                .step(&mut self.world, self.clock.delta_time);
            for event in step.events {
                self.events.push(event);
            }
            for message in step.failures {
                tracing::warn!(target: "script", "{message}");
            }
        }
    }

    /// The camera where the scripts asked this frame, and the scripts told
    /// where it is now. Returns the listener's move for a script-driven
    /// camera (chase cam etc.) that set a position.
    fn apply_script_camera(
        &mut self,
        has_fps_player: bool,
        camera: &mut Camera,
    ) -> Option<Listener> {
        let cam = std::mem::take(&mut self.script_requests.camera);
        super::script_commands::apply_camera_overrides(camera, &cam);
        self.script
            .set_camera(camera.position_array(), camera.forward_vector());
        self.script
            .set_camera_view_proj(camera.view_projection_matrix());
        (!has_fps_player && cam.position.is_some()).then_some(Listener::ScriptCamera)
    }

    /// Animations, then the sprite animations' ends and sequence cues to
    /// the scripts (skipped while the game state pauses animation).
    #[profiling::function]
    fn animate(&mut self, config: &StateConfig) {
        if !config.runs(ANIMATION) {
            return;
        }
        self.animation
            .update(&mut self.world, self.clock.delta_time)
            .ok();

        // Deliver sprite animation end events to scripts
        let sprite_events = self.animation.drain_sprite_events();
        if !sprite_events.is_empty() {
            self.script
                .call_sprite_anim_ends(&mut self.world, &sprite_events);
        }

        // Deliver animation sequence cues to the owning entity's script
        let cues = self.animation.drain_sequence_cues();
        if !cues.is_empty() {
            for cue in &cues {
                println!(
                    "[sequence] {:?} '{}' cue '{}' at {:.2}s",
                    cue.entity_id, cue.sequence, cue.cue, cue.time
                );
            }
            self.script.call_sequence_cues(&mut self.world, &cues);
        }
    }

    /// Bone probes, IK, then the particles.
    fn after_animation(&mut self, config: &StateConfig) {
        // Publish bone_probe joints: model-local joint positions written
        // into the component right after the pose computation, so scripts
        // read this frame's pose (e.g. seat_camera following the eye).
        super::headless_animation::publish_bone_probes(&self.animation, &mut self.world);

        // Two-bone IK (ADR 0070): pin chain tips to their targets now that
        // scripts, physics and animation have all posed the world; the
        // renderer reads world matrices on demand, so this is the last
        // writer it sees.
        self.animation.run_ik(&mut self.world);

        // Advance particle simulation (skip when paused)
        if config.runs(PARTICLES) {
            self.particles
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }
    }

    /// The frame-budgeted procgen queue: what it finished goes to the
    /// renderer, when there is one. Living props move on after it, at
    /// `Stage::Procgen`, in whatever it left of the budget.
    #[profiling::function]
    pub(crate) fn tick_procgen(&mut self, lent: Lent<'_>) {
        let completed = self
            .procgen_resolver
            .process_frame(lent.camera.position_array());
        let failures = self.procgen_resolver.take_failures();
        if completed.is_empty() && failures.is_empty() {
            return;
        }
        if let (Some(renderer), Some((device, queue))) = (lent.renderer, lent.gpu) {
            // Catalog names never reach the queue, so the file fallback
            // needs only the scene's own search paths.
            let config = flint_render::model_loader::ModelLoadConfig::in_project(
                self.project.clone(),
                &self.scene_path,
            );
            let mut sink = super::procgen_assets::GpuSink {
                renderer,
                device,
                queue,
                config: &config,
            };
            super::procgen_assets::apply_completed(&completed, &mut sink);
            super::procgen_assets::apply_failures(&failures, &mut sink);
        }
    }
}
