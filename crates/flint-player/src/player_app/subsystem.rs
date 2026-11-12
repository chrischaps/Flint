//! The player's subsystems (ADR 0117 decision 2, #724 step 3): optional
//! features behind one internal trait, so `PlayerApp` holds a list of them
//! instead of each one's fields, and a scene that doesn't use a feature runs
//! no code for it.
//!
//! A subsystem hears the scene come and go (`scene_loaded`,
//! `scene_unloading`), each [`Stage`] of the frame (`update`), the scripts'
//! commands before the player's own dispatcher (`script_command`), the debug
//! keys (`on_key`), the moment before the frame is encoded (`pre_render`) and
//! the end of an `--exit-after` run (`exit_report`). Every hook has a default
//! that does nothing.
//!
//! **Borrowing.** A hook never sees `PlayerApp`. It gets a context that lends
//! the parts it may touch, each borrowed from its own field: the world, the
//! clock, input, the script system, the camera, the renderer. The list lives
//! in a field of its own (`GameCore::subsystems`), so the player can lend
//! those while it walks the list. What a context holds is what the clients so
//! far have needed, and it grows with the next ones (shallow wooze, living
//! props); see each context's notes.
//!
//! **Lifetimes.** A subsystem lasts for the run (the expression feed, which
//! the host hands over) or for one scene (shallow wooze). The scene ones are
//! built when a scene is entered, by the `for_scene` builders of the files in
//! `player_app/subsystems/` (one file a subsystem, listed by `build.rs`, so
//! adding one edits no other file), each of
//! which looks at the scene and makes its subsystem only if the scene uses
//! it, and they are dropped when the scene unloads. Nothing of one scene's
//! carries into the next.
//!
//! **Order.** Subsystems run in the order they were added. They are torn down
//! the other way round, last added first, at a scene's unloading and when the
//! player drops: [`Subsystems`] drops its members in that order itself, and is
//! declared before `audio` in `PlayerApp`, so whatever one holds of the audio
//! engine goes before the engine (ADR 0017).

use super::{LaunchOptions, SceneSettings};
use flint_audio::engine::AudioEngine;
use flint_core::project::Project;
use flint_ecs::FlintWorld;
use flint_procgen::ProcGenResolver;
use flint_render::{Camera, SceneRenderer};
use flint_runtime::pad::Gilrs;
use flint_runtime::{GameClock, GameStateMachine, InputState, StateConfig};
use flint_script::context::ScriptCommand;
use flint_script::ScriptSystem;
use std::any::Any;
use winit::keyboard::KeyCode;

/// The stages of a frame, in the order `tick` runs them (ADR 0117
/// decision 1). A subsystem's `update` is called at each, after the player's
/// own work of the stage before it and before its own work of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The frame's very start, before the clock ticks, in place of the
    /// player's pad polling while a subsystem has the pads ([`Pads`]): where
    /// the rhythm session drains its capture into input (ADR 0018). Run by
    /// the host, which owns the clock, not by `game_core::step`; a host with
    /// no pads (a capture) skips it.
    Clock,
    /// The clock has ticked and scripted input is in; before the transition
    /// advances and before extensions' `pre_update`. Where outside input for
    /// scripts is queued (the expression feed).
    Input,
    /// After extensions' `pre_update`, before scripts' `on_update`.
    PreScripts,
    /// After `on_update`, before the fixed physics steps.
    Scripts,
    /// After the physics steps and the first-person camera.
    Physics,
    /// After script commands and camera overrides: the camera is final for
    /// the frame.
    PostPhysics,
    /// After `on_draw_ui` and the draw commands.
    ScriptUi,
    /// After audio triggers, animation and particles.
    Animation,
    /// After the procgen queue and living props.
    Procgen,
    /// After the renderer's sync, the frame's post overrides and a
    /// script's quality settings; before the frame's input is cleared.
    Sync,
    /// Before the frame is encoded. Delivered through `pre_render`, with
    /// the renderer, not through `update`; named here so the schedule is
    /// whole in one place.
    #[allow(dead_code)]
    PreRender,
}

impl Stage {
    /// Every stage `update` hears in the player's frame, in order: the
    /// host's [`Stage::Clock`], then `game_core::step`'s.
    #[cfg(test)]
    pub(crate) const UPDATES: [Stage; 10] = [
        Stage::Clock,
        Stage::Input,
        Stage::PreScripts,
        Stage::Scripts,
        Stage::Physics,
        Stage::PostPhysics,
        Stage::ScriptUi,
        Stage::Animation,
        Stage::Procgen,
        Stage::Sync,
    ];
}

/// What a subsystem may touch while a scene comes up or goes down.
#[allow(dead_code)]
pub(crate) struct SceneCtx<'a> {
    pub world: &'a mut FlintWorld,
    pub scene_path: &'a str,
    /// The scene file's top-level tables as written (`[living]`, `[wooze]`,
    /// a game's own). The `SceneFile` itself is gone by the time a scene is
    /// entered; `apply_scene_settings` kept these, and `scene` what it read.
    pub tables: &'a toml::Table,
    /// What the scene's file says, as `apply_scene_settings` read it (with a
    /// `[wooze]` preset filled in, for one).
    pub scene: &'a SceneSettings,
    /// What the host asked for. A subsystem may take what is its own: the
    /// comparison `--compare` asked for, kept across scenes.
    pub launch: &'a mut LaunchOptions,
    pub project: &'a Project,
    pub script: &'a mut ScriptSystem,
    /// The procgen resolver, which holds the scene's living props.
    pub resolver: &'a mut ProcGenResolver,
    /// `None` in a headless test.
    pub renderer: Option<&'a mut SceneRenderer>,
    /// The sound, when the host has it: a subsystem that plays on the
    /// shared kira manager (the music, ADR 0017) starts there.
    pub audio: Option<&'a mut AudioEngine>,
    /// The pads, when the host has them.
    pub pads: Option<Pads<'a>>,
}

/// The host's pads, lent to a subsystem that reads them its own way (the
/// rhythm session's 1 kHz capture thread, ADR 0018): [`Pads::take`] stops
/// the player's polling and its rumble, [`Pads::give_back`] opens them
/// again. A subsystem that takes them gives them back when it goes.
pub(crate) struct Pads<'a> {
    pub(crate) gilrs: &'a mut Option<Gilrs>,
    pub(crate) rumble: &'a mut flint_runtime::PadRumble,
    /// Whether a subsystem holds them: the player then skips its polling.
    pub(crate) lent: &'a mut bool,
}

impl Pads<'_> {
    /// The pads are the subsystem's until it gives them back: the player's
    /// polling stops and a script's rumble does nothing.
    #[allow(dead_code)]
    pub(crate) fn take(&mut self) {
        self.rumble.stop_all();
        *self.gilrs = None;
        *self.lent = true;
    }

    /// The player polls the pads again, through a fresh backend.
    #[allow(dead_code)]
    pub(crate) fn give_back(&mut self) {
        *self.gilrs = Gilrs::new().ok();
        *self.lent = false;
    }
}

/// What a subsystem may touch at a stage of the frame, and at a script
/// command, a key or a click.
#[allow(dead_code)]
pub(crate) struct StageCtx<'a> {
    pub world: &'a mut FlintWorld,
    pub clock: &'a GameClock,
    /// The frame's input, which a subsystem may also write (the rhythm
    /// session's pad drain, at [`Stage::Clock`]).
    pub input: &'a mut InputState,
    pub script: &'a mut ScriptSystem,
    /// The camera as it stands at this stage (final from `PostPhysics` on).
    pub camera: &'a Camera,
    /// Which systems the game state lets run this frame. `None` at
    /// `Stage::Input`, which runs before the frame reads it (a transition
    /// may still change the state that frame), and outside a stage. A
    /// subsystem asks by its `name()`: [`StageCtx::runs`].
    pub policy: Option<&'a StateConfig>,
    /// `None` in a headless test.
    pub renderer: Option<&'a mut SceneRenderer>,
    /// The device and queue, when there is a window.
    pub gpu: Option<(&'a wgpu::Device, &'a wgpu::Queue)>,
    /// The procgen resolver, which holds the scene's living props and makes
    /// their poses on its frame budget.
    pub resolver: &'a mut ProcGenResolver,
    pub project: &'a Project,
    pub scene_path: &'a str,
    /// Whether the scene file may be written (#364).
    pub scene_file_writable: bool,
    /// egui's points per pixel, for a pointer in window pixels.
    pub pixels_per_point: f32,
    pub cursor_captured: bool,
    /// What a debug key says when it has nothing to act on (#367): on
    /// screen for a moment and in the console.
    pub note: &'a mut super::key_note::KeyNote,
    /// The F3/F4 panels, for a subsystem that has one.
    #[cfg(feature = "debug-hud")]
    pub panels: &'a mut Vec<Box<dyn flint_debug_ui::DebugPanel>>,
    /// What scripts asked of the frame, before the host applies it: a
    /// subsystem may fill what they left (the rhythm ladder's post values,
    /// at [`Stage::Procgen`], before the renderer's sync).
    pub requests: &'a mut flint_script::FrameRequests,
    /// The sound, when the host has it.
    pub audio: Option<&'a mut AudioEngine>,
    /// The pads, when the host has them.
    pub pads: Option<Pads<'a>>,
    /// Set to end the run as closing the window would (`quit_on_finish`);
    /// `None` for a host with no run to end.
    pub quit: Option<&'a mut bool>,
}

impl StageCtx<'_> {
    /// Whether the game state lets the system `name` run this frame (a
    /// subsystem asks by its own `name()`). Outside a stage, it does.
    pub(crate) fn runs(&self, name: &str) -> bool {
        self.policy.is_none_or(|policy| policy.runs(name))
    }
}

/// A key the debug keys hear (a real press or a scripted one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyPress {
    pub code: KeyCode,
    pub shift: bool,
}

/// Input a subsystem may take before the game and the player see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerInput {
    /// A mouse button, real or scripted, before egui's own use of it.
    Click {
        state: winit::event::ElementState,
        button: winit::event::MouseButton,
    },
    /// A real Escape, before it releases the cursor or quits.
    Escape,
}

/// What a subsystem may touch after the frame's scene is encoded and before
/// the HUD: a second look at the scene drawn over part of the frame (the
/// Living Props A/B split, #238).
pub(crate) struct EncodeCtx<'a> {
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// The frame's texture, and its size
    pub output: &'a wgpu::Texture,
    pub depth_view: &'a wgpu::TextureView,
    /// The scene's and the HUD's formats of the frame
    pub formats: (wgpu::TextureFormat, wgpu::TextureFormat),
    pub renderer: &'a mut SceneRenderer,
    pub world: &'a FlintWorld,
    pub camera: &'a Camera,
    pub resolver: &'a mut ProcGenResolver,
}

/// What a subsystem may read for the `--exit-after` report.
pub(crate) struct ReportCtx<'a> {
    pub resolver: &'a ProcGenResolver,
    pub renderer: Option<&'a SceneRenderer>,
}

/// How long a subsystem lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lasts {
    /// The whole run: the host made it (the expression feed).
    Run,
    /// One scene: built by its `for_scene` builder when a scene that uses it
    /// is entered, dropped when that scene unloads.
    Scene,
}

// The scene subsystems' modules, one a file of `player_app/subsystems/`,
// `for_scene_builders()`, their builders in file-name order, and
// `subsystem_policies()`, each file's `POLICY` (build.rs). Each builder looks
// at the scene being entered and makes its subsystem only if the scene uses
// it.
include!(concat!(env!("OUT_DIR"), "/subsystems.rs"));

/// The player's game-state machine: a state may name the built-in systems
/// and every scene subsystem that reads its policy (its file's `POLICY`),
/// whether or not this scene has it, so a state registered in one scene
/// pauses the grove of the next. Any other name is `register_state`'s error
/// (#727).
pub(crate) fn state_machine() -> GameStateMachine {
    let mut machine = GameStateMachine::new();
    for name in subsystem_policies() {
        machine.add_system(name);
    }
    machine
}

/// A scene subsystem's builder: its subsystem, if the scene uses it.
pub(crate) type BuildForScene = fn(&mut SceneCtx) -> Option<Box<dyn Subsystem>>;

/// What a subsystem may touch before the frame is encoded.
#[allow(dead_code)]
pub(crate) struct RenderCtx<'a> {
    pub renderer: &'a mut SceneRenderer,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub camera: &'a Camera,
    pub clock: &'a GameClock,
}

/// Lines a subsystem adds to the `--exit-after` report, printed after the
/// player's own.
#[derive(Debug, Default)]
pub(crate) struct ExitReport {
    pub lines: Vec<String>,
}

impl ExitReport {
    #[allow(dead_code)] // no client reports yet
    pub(crate) fn line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }
}

/// An optional part of the player (ADR 0117 decision 2). Internal for now;
/// `PlayerExtension` gains the same hooks once the engine's own features
/// have shown what they must carry (decision 5).
pub(crate) trait Subsystem: Any {
    /// For logs and the debug panels, and the name a game state's policy
    /// pauses it by when its file lists it as its `POLICY` (#727).
    fn name(&self) -> &'static str;

    /// For one scene (the default) or the whole run.
    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    /// The scene is up: its world, settings and renderer state are in and
    /// its scripts have run `on_init` (the last step of `enter_scene`).
    fn scene_loaded(&mut self, _ctx: &mut SceneCtx) {}

    /// The scene is going: its scripts have heard `on_scene_exit`, nothing
    /// has been cleared yet.
    fn scene_unloading(&mut self, _ctx: &mut SceneCtx) {}

    /// A stage of the frame. See [`Stage`] for where each falls.
    fn update(&mut self, _stage: Stage, _ctx: &mut StageCtx) {}

    /// The script modules this subsystem owns and enables while it runs
    /// (#698): their functions are a "not enabled" error in a scene
    /// without it. The modules themselves are declared from its file's
    /// `SCRIPT_MODULES`, once, for every scene.
    fn script_modules(&self) -> &[&'static str] {
        &[]
    }

    /// A script's command, before the player's dispatcher sees it. Return
    /// `true` to take it: the player then never applies it.
    fn script_command(&mut self, _command: &ScriptCommand, _ctx: &mut StageCtx) -> bool {
        false
    }

    /// A debug key (a real press or a scripted one). Return `true` to take
    /// it from the player's own debug keys; the game still sees the key.
    fn on_key(&mut self, _key: KeyPress, _ctx: &mut StageCtx) -> bool {
        false
    }

    /// A click or a real Escape, before anything else hears it. Return `true`
    /// to take it: then nothing else does.
    fn on_input(&mut self, _input: PlayerInput, _ctx: &mut StageCtx) -> bool {
        false
    }

    /// Before the frame is encoded, with the renderer.
    fn pre_render(&mut self, _ctx: &mut RenderCtx) {}

    /// After the frame's scene is encoded, before the HUD.
    fn post_encode(&mut self, _ctx: &mut EncodeCtx) {}

    /// The HUD's egui pass, over the game's UI and under the debug panels.
    fn ui(&mut self, _egui: &egui::Context) {}

    /// The end of an `--exit-after` run.
    fn exit_report(&self, _ctx: &ReportCtx, _report: &mut ExitReport) {}
}

/// The player's subsystems, in the order they were added, torn down in the
/// other order.
#[derive(Default)]
pub(crate) struct Subsystems {
    list: Vec<Box<dyn Subsystem>>,
}

impl Subsystems {
    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Add one; it runs after those already here and is torn down before
    /// them.
    pub(crate) fn push(&mut self, subsystem: Box<dyn Subsystem>) {
        tracing::debug!("subsystem {} added", subsystem.name());
        self.list.push(subsystem);
    }

    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.list.iter().map(|s| s.name()).collect()
    }

    /// Drop the scene's subsystems, last added first, keeping the run's.
    pub(crate) fn drop_scene_ones(&mut self) {
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            if self.list[i].lasts() == Lasts::Scene {
                let gone = self.list.remove(i);
                tracing::debug!("subsystem {} dropped with its scene", gone.name());
            }
        }
    }

    /// The subsystem of type `T`, if there is one.
    pub(crate) fn find<T: Subsystem>(&self) -> Option<&T> {
        self.list
            .iter()
            .find_map(|s| (s.as_ref() as &dyn Any).downcast_ref::<T>())
    }

    /// The subsystem of type `T`, if there is one, to change.
    pub(crate) fn find_mut<T: Subsystem>(&mut self) -> Option<&mut T> {
        self.list
            .iter_mut()
            .find_map(|s| (s.as_mut() as &mut dyn Any).downcast_mut::<T>())
    }

    pub(crate) fn scene_loaded(&mut self, ctx: &mut SceneCtx) {
        for s in &mut self.list {
            s.scene_loaded(ctx);
        }
    }

    /// Last added first, as a teardown goes.
    pub(crate) fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        for s in self.list.iter_mut().rev() {
            s.scene_unloading(ctx);
        }
    }

    pub(crate) fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        for s in &mut self.list {
            s.update(stage, ctx);
        }
    }

    /// Every module the subsystems here enable (#698).
    pub(crate) fn script_modules(&self) -> Vec<&'static str> {
        let mut modules: Vec<&'static str> = Vec::new();
        for s in &self.list {
            for m in s.script_modules() {
                if !modules.contains(m) {
                    modules.push(m);
                }
            }
        }
        modules
    }

    /// Whether a subsystem took `command`.
    pub(crate) fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        self.list.iter_mut().any(|s| s.script_command(command, ctx))
    }

    pub(crate) fn on_key(&mut self, key: KeyPress, ctx: &mut StageCtx) -> bool {
        self.list.iter_mut().any(|s| s.on_key(key, ctx))
    }

    pub(crate) fn on_input(&mut self, input: PlayerInput, ctx: &mut StageCtx) -> bool {
        self.list.iter_mut().any(|s| s.on_input(input, ctx))
    }

    pub(crate) fn post_encode(&mut self, ctx: &mut EncodeCtx) {
        for s in &mut self.list {
            s.post_encode(ctx);
        }
    }

    pub(crate) fn ui(&mut self, egui: &egui::Context) {
        for s in &mut self.list {
            s.ui(egui);
        }
    }

    pub(crate) fn pre_render(&mut self, ctx: &mut RenderCtx) {
        for s in &mut self.list {
            s.pre_render(ctx);
        }
    }

    pub(crate) fn exit_report(&self, ctx: &ReportCtx, report: &mut ExitReport) {
        for s in &self.list {
            s.exit_report(ctx, report);
        }
    }
}

impl Drop for Subsystems {
    /// Last added first, the order `scene_unloading` runs in.
    fn drop(&mut self) {
        while let Some(last) = self.list.pop() {
            drop(last);
        }
    }
}

/// A [`StageCtx`] over `$app`'s fields, borrowed one by one, so the
/// subsystem list (another field) can be walked while it is lent.
macro_rules! stage_ctx {
    ($app:expr, $policy:expr) => {
        $crate::player_app::subsystem::StageCtx {
            world: &mut $app.core.world,
            clock: &$app.core.clock,
            input: &mut $app.input,
            script: &mut $app.core.script,
            camera: &$app.camera,
            policy: $policy,
            renderer: $app.scene_renderer.as_mut(),
            gpu: $app.render_context.as_ref().map(|c| (&c.device, &c.queue)),
            resolver: &mut $app.core.procgen_resolver,
            project: &$app.core.project,
            scene_path: &$app.core.scene_path,
            scene_file_writable: $app.launch.scene_file_writable,
            pixels_per_point: $app.hud.egui_ctx.pixels_per_point(),
            cursor_captured: $app.focus.cursor_captured,
            note: &mut $app.hud.key_note,
            #[cfg(feature = "debug-hud")]
            panels: &mut $app.debug_panels,
            requests: &mut $app.core.script_requests,
            audio: Some(&mut $app.audio.engine),
            pads: Some($crate::player_app::subsystem::Pads {
                gilrs: &mut $app.gilrs,
                rumble: &mut $app.pad_rumble,
                lent: &mut $app.pads_lent,
            }),
            quit: Some(&mut $app.outcome.exit_requested),
        }
    };
}
#[allow(unused_imports)]
pub(crate) use stage_ctx;

/// A [`SceneCtx`] over `$app`'s fields.
macro_rules! scene_ctx {
    ($app:expr) => {
        $crate::player_app::subsystem::SceneCtx {
            world: &mut $app.core.world,
            scene_path: &$app.core.scene_path,
            tables: &$app.scene.tables,
            scene: &$app.scene,
            launch: &mut $app.launch,
            project: &$app.core.project,
            script: &mut $app.core.script,
            resolver: &mut $app.core.procgen_resolver,
            renderer: $app.scene_renderer.as_mut(),
            audio: Some(&mut $app.audio.engine),
            pads: Some($crate::player_app::subsystem::Pads {
                gilrs: &mut $app.gilrs,
                rumble: &mut $app.pad_rumble,
                lent: &mut $app.pads_lent,
            }),
        }
    };
}

/// What the player does with its subsystems: building the list for a
/// scene, and lending each hook its context.
impl super::PlayerApp {
    /// The subsystems a scene being entered has: those the run brought (the
    /// expression feed, at `Start`) and this scene's own, each only if the
    /// scene uses it, with the script modules they own enabled (#698).
    /// Called before the scene's scripts load, so their `on_init` can call
    /// those modules, as a capture's can (`HeadlessGame::start_scripts`);
    /// they hear `scene_loaded` once the scripts are up.
    pub(super) fn subsystems_build(&mut self, entry: super::scene_entry::SceneEntry) {
        if entry == super::scene_entry::SceneEntry::Start {
            // The feed belongs to the run (#518), so it outlives every scene
            if let Some(feed) = self.launch.expression_feed.take() {
                self.core.subsystems.push(Box::new(
                    super::expression_feed::ExpressionSubsystem::new(feed),
                ));
            }
        }
        let mut ctx = scene_ctx!(self);
        for build in for_scene_builders() {
            if let Some(subsystem) = build(&mut ctx) {
                self.core.subsystems.push(subsystem);
            }
        }
        self.core.enable_script_modules();
    }

    /// The scene is up: each subsystem hears `scene_loaded`. Called last in
    /// `enter_scene`, once the scene's systems and scripts are up.
    pub(super) fn subsystems_enter_scene(&mut self) {
        if self.core.subsystems.is_empty() {
            return;
        }
        let mut ctx = scene_ctx!(self);
        self.core.subsystems.scene_loaded(&mut ctx);
    }

    /// The scene is going: each subsystem hears `scene_unloading`, last
    /// added first, before anything is cleared, and the scene's own go.
    pub(super) fn subsystems_leave_scene(&mut self) {
        if self.core.subsystems.is_empty() {
            return;
        }
        let mut ctx = scene_ctx!(self);
        self.core.subsystems.scene_unloading(&mut ctx);
        self.core.subsystems.drop_scene_ones();
        self.core.enable_script_modules();
    }

    /// Whether a subsystem took a debug key.
    pub(super) fn subsystems_take_key(&mut self, key: KeyPress) -> bool {
        if self.core.subsystems.is_empty() {
            return false;
        }
        let mut ctx = stage_ctx!(self, None);
        self.core.subsystems.on_key(key, &mut ctx)
    }

    /// Whether a subsystem took a click or an Escape.
    pub(super) fn subsystems_take_input(&mut self, input: PlayerInput) -> bool {
        if self.core.subsystems.is_empty() {
            return false;
        }
        let mut ctx = stage_ctx!(self, None);
        self.core.subsystems.on_input(input, &mut ctx)
    }

    /// The subsystems' lines for the `--exit-after` report.
    pub(super) fn subsystem_report(&self) -> Vec<String> {
        let mut report = ExitReport::default();
        let ctx = ReportCtx {
            resolver: &self.core.procgen_resolver,
            renderer: self.scene_renderer.as_ref(),
        };
        self.core.subsystems.exit_report(&ctx, &mut report);
        report.lines
    }
}
