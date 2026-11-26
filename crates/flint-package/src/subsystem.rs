//! The trait a package's running part implements (ADR 0117 decision 2,
//! made public by ADR 0137).

use crate::{
    EncodeCtx, ExitReport, FixedCtx, FrameCtx, Lasts, PlayerInput, ReportCtx, SceneCtx, Stage,
    StageCtx,
};
use flint_render::FrameInputs;
use flint_runtime::DebugKey;
use flint_script::context::ScriptCommand;
use std::any::Any;

/// An optional part of the player (ADR 0117 decision 2): one a scene uses,
/// built for it and dropped with it, or one the host hands over for the run.
/// Every hook has a default that does nothing.
///
/// Subsystems run in the order they were added and are torn down the other
/// way round, last added first.
pub trait Subsystem: Any {
    /// For logs and the debug panels, and the name a game state's policy
    /// pauses it by (#727).
    fn name(&self) -> &'static str;

    /// For one scene (the default) or the whole run.
    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    /// The scene is up: its world, settings and renderer state are in and
    /// its scripts have run `on_init` (the last step of the scene's entry).
    fn scene_loaded(&mut self, _ctx: &mut SceneCtx) {}

    /// The scene is going: its scripts have heard `on_scene_exit`, nothing
    /// has been cleared yet.
    fn scene_unloading(&mut self, _ctx: &mut SceneCtx) {}

    /// A stage of the frame. See [`Stage`] for where each falls.
    fn update(&mut self, _stage: Stage, _ctx: &mut StageCtx) {}

    /// One fixed physics step ([`Stage::FixedStep`], ADR 0137 section 3a):
    /// once for each step that steps, so twice in a frame at 30 Hz and on
    /// most frames none at 144. It runs after the character controller and
    /// the physics sync-in, before Rapier's step, which integrates what it
    /// applies. A paused state (one that stops physics) skips it, as it
    /// skips Rapier, and so does a capture, which runs no physics.
    fn fixed_update(&mut self, _ctx: &mut FixedCtx) {}

    /// The script modules this subsystem owns and enables while it runs
    /// (#698): their functions are a "not enabled" error in a scene
    /// without it. The modules themselves are declared once, for every
    /// scene.
    fn script_modules(&self) -> &[&'static str] {
        &[]
    }

    /// A script's command, before the player's dispatcher sees it. Return
    /// `true` to take it: the player then never applies it.
    fn script_command(&mut self, _command: &ScriptCommand, _ctx: &mut StageCtx) -> bool {
        false
    }

    /// The debug keys it acts on (#717). While it is in the scene the
    /// table dispatches them to [`on_debug_key`](Self::on_debug_key) and
    /// the game never sees the press; with it gone they are the game's.
    fn debug_keys(&self) -> &'static [DebugKey] {
        &[]
    }

    /// One of its [`debug_keys`](Self::debug_keys) was pressed (real or
    /// scripted). `key` is the chord it is on, as a note names it.
    fn on_debug_key(&mut self, _action: &str, _key: &str, _ctx: &mut StageCtx) {}

    /// A click or a real Escape, before anything else hears it. Return `true`
    /// to take it: then nothing else does.
    fn on_input(&mut self, _input: PlayerInput, _ctx: &mut StageCtx) -> bool {
        false
    }

    /// Its share of the frame's renderer inputs (#739): what it draws this
    /// frame that the world doesn't say (the living subsystem's boil,
    /// shallow wooze and woozy grass). Called once a frame, just before the
    /// renderer's `prepare`; what it leaves at the default is off.
    fn frame_inputs<'s>(&'s self, _ctx: &FrameCtx, _inputs: &mut FrameInputs<'s>) {}

    /// After the frame's scene is encoded, before the HUD.
    fn post_encode(&mut self, _ctx: &mut EncodeCtx) {}

    /// The HUD's egui pass, on the frame's root `Ui` (as
    /// `PlayerExtension::ui`, ADR 0121), over the game's UI and under the
    /// debug panels.
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    /// The end of an `--exit-after` run.
    fn exit_report(&self, _ctx: &ReportCtx, _report: &mut ExitReport) {}

    /// Why a capture (`flint render --scripts`) leaves it out, for a
    /// subsystem that is the window's alone; `None`, the default, runs it
    /// there as in the player (#1252). A capture asks once it is built for
    /// the scene, so it names only what the scene would have had.
    fn presentation_only(&self) -> Option<&'static str> {
        None
    }
}
