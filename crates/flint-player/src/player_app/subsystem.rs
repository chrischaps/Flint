//! The player's subsystems (ADR 0117 decision 2, #724 step 3): optional
//! features behind one trait, so `PlayerApp` holds a list of them instead of
//! each one's fields, and a scene that doesn't use a feature runs no code for
//! it. The trait, [`Stage`] and the contexts are the Package API's
//! (`flint_package`, ADR 0137, #1550); this file keeps the ordered list
//! ([`Subsystems`]), the builders and the lending of each context.
//!
//! A subsystem hears the scene come and go (`scene_loaded`,
//! `scene_unloading`), each [`Stage`] of the frame (`update`), each fixed
//! physics step (`fixed_update`, lent a [`FixedCtx`], #1551), the scripts'
//! commands before the player's own dispatcher (`script_command`), the debug
//! keys it declares (`debug_keys`, `on_debug_key`), a click or an Escape
//! (`on_input`), the frame's renderer inputs and its encoder (`frame_inputs`,
//! `post_encode`), the HUD (`ui`) and the end of an `--exit-after` run
//! (`exit_report`). Every hook has a default that does nothing.
//!
//! **Borrowing.** A hook never sees `PlayerApp`. It gets a context that lends
//! the parts it may touch, each borrowed from its own field: the world, the
//! clock, input, the script system, the camera, the renderer. The list lives
//! in a field of its own (`GameCore::subsystems`), so the player can lend
//! those while it walks the list. Every field is a kernel type: what a
//! package owns is in the run's `Resources` (`GameCore::resources`), which
//! every context lends (the procgen resolver, the living props, the debug
//! panels).
//!
//! **Lifetimes.** A subsystem lasts for the run (the expression feed, which
//! the host hands over) or for one scene (shallow wooze). The scene ones are
//! built when a scene is entered, by the builders the run's packages
//! registered (`flint_package::Registry::scene_subsystem`, #1552: the
//! launcher's list, `crate::packages::registry`), each of which looks at
//! the scene and makes its subsystem only if the scene uses it, and they
//! are dropped when the scene unloads. Nothing of one scene's carries into
//! the next.
//!
//! **Captures.** `flint render --scripts` builds its scene's subsystems from
//! the same list (#1252): `HeadlessGame::start_subsystems` hands each builder
//! a [`SceneCtx`] whose `capture` says what the capture asked for, so a
//! package's subsystem runs in a capture as it does in the player, with
//! nothing else to edit. A builder that builds otherwise there (a grove posed at once, music
//! on the game clock) asks `ctx.capture`; a subsystem that is the window's
//! alone says so from [`Subsystem::presentation_only`], and the capture
//! drops it and names it once in its notes.
//!
//! **Order.** Subsystems run in the order they were added. They are torn down
//! the other way round, last added first, at a scene's unloading and when the
//! player drops: [`Subsystems`] drops its members in that order itself, and is
//! declared before `audio` in `PlayerApp`, so whatever one holds of the audio
//! engine goes before the engine (ADR 0017).

pub(crate) use flint_package::{
    EncodeCtx, ExitReport, FixedCtx, FrameCtx, Lasts, Pads, PlayerInput, ReportCtx, SceneCtx,
    Stage, StageCtx, Subsystem,
};
use flint_render::FrameInputs;
use flint_runtime::DebugKey;
use flint_runtime::GameStateMachine;
use flint_script::context::ScriptCommand;

// The scene subsystems whose code is still in this crate: the kernel's
// `field`; every package's is its own crate's (music's since #1555,
// rhythm's since #1556, living's since #1558). Each file registers what it
// brings through the run's `Registry` (#1552); nothing lists the files.
#[path = "subsystems/field.rs"]
pub(crate) mod field;
#[cfg(test)]
#[path = "subsystems/toy.rs"]
pub(crate) mod toy;

/// The player's game-state machine: a state may name the built-in systems
/// and every name a package registered as its policy (`living`), whether
/// or not this scene has it, so a state registered in one scene pauses the
/// grove of the next. Any other name is `register_state`'s error (#727).
pub(crate) fn state_machine() -> GameStateMachine {
    let mut machine = GameStateMachine::new();
    for name in crate::packages::registry().policies() {
        machine.add_system(name);
    }
    machine
}

/// A winit click as the Package API carries it (ADR 0137 section 3: winit
/// is not in the API).
pub(crate) fn click_of(
    state: winit::event::ElementState,
    button: winit::event::MouseButton,
) -> PlayerInput {
    use flint_package::Button;
    use winit::event::MouseButton;
    PlayerInput::Click {
        button: match button {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
            MouseButton::Back => Button::Back,
            MouseButton::Forward => Button::Forward,
            MouseButton::Other(n) => Button::Other(n),
        },
        pressed: state == winit::event::ElementState::Pressed,
    }
}

/// Every scene subsystem `ctx`'s scene uses, built by the builders the
/// run's packages registered, in the launcher's order: the player's list
/// and a capture's alike. A package the project's `[packages]` leaves out
/// builds nothing, and a scene that would have used it hears the line the
/// package registered for that, once (#1528, ADR 0137 decision 8).
pub(crate) fn build_for_scene(ctx: &mut SceneCtx) -> Vec<Box<dyn Subsystem>> {
    let registry = crate::packages::registry();
    let mut built = Vec::new();
    let mut said: Vec<&str> = Vec::new();
    for (package, build) in registry.builders() {
        if ctx.leaves_out(package).is_some() {
            if !said.contains(&package) {
                said.push(package);
                for line in registry.left_out_lines(package) {
                    if let Some(line) = line(ctx) {
                        tracing::warn!(target: "load", "{line}");
                    }
                }
            }
            continue;
        }
        built.extend(build(ctx));
    }
    built
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

    /// The subsystem of type `T`, if there is one (the native hosts': the
    /// world dump, the expression feed's socket).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn find<T: Subsystem>(&self) -> Option<&T> {
        flint_package::find(&self.list)
    }

    /// The subsystem of type `T`, if there is one, to change.
    #[cfg_attr(not(test), allow(dead_code))] // the living tests' (#754)
    pub(crate) fn find_mut<T: Subsystem>(&mut self) -> Option<&mut T> {
        flint_package::find_mut(&mut self.list)
    }

    /// Every one, in run order: a capture's, for a package's CLI side to
    /// find its own in (`flint_package::CaptureGame`, #1558).
    pub(crate) fn as_slice(&self) -> &[Box<dyn Subsystem>] {
        &self.list
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [Box<dyn Subsystem>] {
        &mut self.list
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

    /// One fixed physics step, in the order they were added (#1551).
    pub(crate) fn fixed_update(&mut self, ctx: &mut FixedCtx) {
        for s in &mut self.list {
            s.fixed_update(ctx);
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

    /// Whether one of them declares the debug action `action`.
    pub(crate) fn declares(&self, action: &str) -> bool {
        self.list
            .iter()
            .any(|s| s.debug_keys().iter().any(|k| k.action == action))
    }

    /// Each one's name and debug keys, in run order.
    pub(crate) fn debug_keys(
        &self,
    ) -> impl Iterator<Item = (&'static str, &'static [DebugKey])> + '_ {
        self.list.iter().map(|s| (s.name(), s.debug_keys()))
    }

    /// Hand `action` to the first that declares it.
    pub(crate) fn on_debug_key(&mut self, action: &str, key: &str, ctx: &mut StageCtx) {
        if let Some(s) = self
            .list
            .iter_mut()
            .find(|s| s.debug_keys().iter().any(|k| k.action == action))
        {
            s.on_debug_key(action, key, ctx);
        }
    }

    pub(crate) fn on_input(&mut self, input: PlayerInput, ctx: &mut StageCtx) -> bool {
        self.list.iter_mut().any(|s| s.on_input(input, ctx))
    }

    pub(crate) fn post_encode(&mut self, ctx: &mut EncodeCtx) {
        for s in &mut self.list {
            s.post_encode(ctx);
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) {
        for s in &mut self.list {
            s.ui(ui);
        }
    }

    pub(crate) fn frame_inputs<'s>(&'s self, ctx: &FrameCtx, inputs: &mut FrameInputs<'s>) {
        for s in &self.list {
            s.frame_inputs(ctx, inputs);
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

/// `$body` with `$ctx` a [`StageCtx`] over `$app`'s fields, borrowed one
/// by one, so the subsystem list (another field) can be walked while it is
/// lent. The sound is the run's resource (#1619), lent with the rest.
macro_rules! with_stage_ctx {
    ($app:expr, $policy:expr, |$ctx:ident| $body:expr) => {{
        #[allow(unused_mut)]
        let mut $ctx = $crate::player_app::subsystem::StageCtx {
            world: &mut $app.core.world,
            clock: &$app.core.clock,
            input: &mut $app.input,
            script: &mut $app.core.script,
            camera: &$app.camera,
            policy: $policy,
            renderer: $app.scene_renderer.as_mut(),
            gpu: $app.render_context.as_ref().map(|c| (&c.device, &c.queue)),
            project: &$app.core.project,
            scene_path: &$app.core.scene_path,
            scene_file_writable: $app.launch.scene_file_writable,
            pixels_per_point: $app.hud.egui_ctx.pixels_per_point(),
            cursor_captured: $app.focus.cursor_captured,
            note: &mut $app.hud.key_note,
            requests: &mut $app.core.script_requests,
            pads: Some($crate::player_app::subsystem::Pads::new(
                &mut $app.gilrs,
                &mut $app.pad_rumble,
                &mut $app.pads_lent,
            )),
            quit: Some(&mut $app.outcome.exit_requested),
            resources: &mut $app.core.resources,
            events: &[],
        };
        $body
    }};
}
#[allow(unused_imports)]
pub(crate) use with_stage_ctx;

/// `$body` with `$ctx` a [`SceneCtx`] over `$app`'s fields.
macro_rules! with_scene_ctx {
    ($app:expr, |$ctx:ident| $body:expr) => {{
        #[allow(unused_mut)]
        let mut $ctx = $crate::player_app::subsystem::SceneCtx {
            world: &mut $app.core.world,
            scene_path: &$app.core.scene_path,
            tables: &$app.scene.tables,
            project: &$app.core.project,
            styles: &$app.core.styles,
            post: $app.scene.post_process.as_ref(),
            script: &mut $app.core.script,
            renderer: $app.scene_renderer.as_mut(),
            pads: Some($crate::player_app::subsystem::Pads::new(
                &mut $app.gilrs,
                &mut $app.pad_rumble,
                &mut $app.pads_lent,
            )),
            capture: None,
            packages: $app.core.packages.as_ref(),
            resources: &mut $app.core.resources,
            preload_audio: $app.scene.preload_audio,
            registry: $crate::packages::registry(),
        };
        $body
    }};
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
            // The live world's socket and its file (#713), the run's too
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(listener) = self.launch.inspect.take() {
                    self.core
                        .subsystems
                        .push(Box::new(super::inspect::InspectSubsystem::new(listener)));
                }
                if let Some(dump) = self.launch.world_dump.take() {
                    self.core.subsystems.push(Box::new(dump));
                }
            }
        }
        let built = with_scene_ctx!(self, |ctx| build_for_scene(&mut ctx));
        for subsystem in built {
            self.core.subsystems.push(subsystem);
        }
        self.core.enable_script_modules();
    }

    /// The scene is up: each subsystem hears `scene_loaded`, then each of
    /// the game's extensions (#754). Called last in `enter_scene`, once the
    /// scene's systems and scripts are up.
    pub(super) fn subsystems_enter_scene(&mut self) {
        if !self.core.subsystems.is_empty() {
            with_scene_ctx!(self, |ctx| self.core.subsystems.scene_loaded(&mut ctx));
        }
        self.each_extension(|ext, ctx| {
            ext.scene_loaded(ctx);
            false
        });
    }

    /// The scene is going: the game's extensions hear `scene_unloading`
    /// (#754), then each subsystem, last added first, before anything is
    /// cleared, and the scene's own go.
    pub(super) fn subsystems_leave_scene(&mut self) {
        self.each_extension(|ext, ctx| {
            ext.scene_unloading(ctx);
            false
        });
        if !self.core.subsystems.is_empty() {
            with_scene_ctx!(self, |ctx| self.core.subsystems.scene_unloading(&mut ctx));
            self.core.subsystems.drop_scene_ones();
            self.core.enable_script_modules();
        }
    }

    /// Hand a subsystem the debug action its key was pressed for.
    pub(super) fn subsystems_debug_key(&mut self, action: &str, key: &str) {
        if self.core.subsystems.is_empty() {
            return;
        }
        with_stage_ctx!(self, None, |ctx| {
            self.core.subsystems.on_debug_key(action, key, &mut ctx)
        });
    }

    /// Whether a subsystem took a click or an Escape.
    pub(super) fn subsystems_take_input(&mut self, input: PlayerInput) -> bool {
        if self.core.subsystems.is_empty() {
            return false;
        }
        with_stage_ctx!(self, None, |ctx| {
            self.core.subsystems.on_input(input, &mut ctx)
        })
    }

    /// The subsystems' lines for the `--exit-after` report.
    pub(super) fn subsystem_report(&self) -> Vec<String> {
        let mut report = ExitReport::default();
        let ctx = ReportCtx {
            renderer: self.scene_renderer.as_ref(),
            resources: &self.core.resources,
        };
        self.core.subsystems.exit_report(&ctx, &mut report);
        report.lines
    }
}
