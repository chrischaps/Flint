//! What a subsystem's hooks are lent (ADR 0117 decision 2, ADR 0137
//! section 3). A hook never sees the player: it gets a context that lends
//! the parts it may touch, each borrowed from its own field, so the host can
//! lend them while it walks the subsystem list (another field).
//!
//! Every field is a kernel type. What a package owns, and another package
//! reads, is in [`Resources`]: procgen's resolver, living's props, the
//! debug-hud's panels.

use crate::{Note, Registry, Resources, Subsystem};
use flint_audio::engine::AudioEngine;
use flint_core::package::InUse;
use flint_core::project::Project;
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use flint_render::model_loader::ModelLoadResult;
use flint_render::stylize::StyleRegistry;
use flint_render::{Camera, SceneRenderer};
use flint_runtime::pad::Gilrs;
use flint_runtime::{EventBus, GameClock, InputState, PadRumble, StateConfig};
use flint_script::{FrameRequests, ScriptSystem};

/// What a capture (`flint render --scripts`, #1252) asks of a scene's
/// subsystems, lent as [`SceneCtx::capture`]. A package's own capture
/// inputs (living's wooze field) are in [`Resources`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capture {
    /// A capture of a moment, which draws shallow wooze.
    pub moment: bool,
    /// Whether the scene's scripts run: the music and the declared fields
    /// are only the scripts' to read, and are built only for them.
    pub scripts: bool,
}

/// What a subsystem may touch while a scene comes up or goes down.
pub struct SceneCtx<'a> {
    pub world: &'a mut FlintWorld,
    pub scene_path: &'a str,
    /// The scene file's top-level tables as written (`[living]`, `[wooze]`,
    /// a game's own). The scene file itself is gone by the time a scene is
    /// entered.
    pub tables: &'a toml::Table,
    pub project: &'a Project,
    /// The styles the scene is drawn with (#742).
    pub styles: &'a StyleRegistry,
    /// The scene's `[post_process]` as the host read it, its look laid in
    /// (`None`: the defaults, or a capture). The rhythm ladder's base
    /// (#1556): what a package reads of the scene's look without its tables.
    pub post: Option<&'a flint_scene::PostProcessDef>,
    pub script: &'a mut ScriptSystem,
    /// `None` in a headless test and a capture.
    pub renderer: Option<&'a mut SceneRenderer>,
    /// The sound, when the host has it: a subsystem that plays on the
    /// shared kira manager (the music, ADR 0017) starts there.
    pub audio: Option<&'a mut AudioEngine>,
    /// The pads, when the host has them.
    pub pads: Option<Pads<'a>>,
    /// `Some` when the host is a capture (`flint render`'s `HeadlessGame`,
    /// #1252): what it asked for. The player's is `None`.
    pub capture: Option<&'a Capture>,
    /// The packages the project's `[packages]` lets in (#1334); `None`:
    /// every package is in. A builder of a package it leaves out makes
    /// nothing and says so ([`SceneCtx::leaves_out`], #1528).
    pub packages: Option<&'a InUse>,
    /// What the model loader made of the scene's models, when the host
    /// loaded them for this scene: the skeletons and clips a skinned or
    /// animated model brought, and which entities are skinned (#1620).
    /// `None` in a headless test and a capture, whose animation is its own
    /// ([`CapturePose`]). A model spawned later reaches the subsystems
    /// through [`Subsystem::late_models`](crate::Subsystem::late_models).
    pub models: Option<&'a ModelLoadResult>,
    /// The run's resources: what one package lends another.
    pub resources: &'a mut Resources,
    /// What the run's packages registered: a builder reads another
    /// package's registration point here (music's chart judge, which
    /// rhythm fills, #1555), and asks whether a package is in the build.
    pub registry: &'a Registry,
}

impl SceneCtx<'_> {
    /// The project's `[packages]` leaves `package` out: its builder makes
    /// nothing, and says so once.
    pub fn leaves_out(&self, package: &str) -> Option<&InUse> {
        self.packages.filter(|in_use| !in_use.is_on(package))
    }
}

/// The host's pads, lent to a subsystem that reads them its own way (the
/// rhythm session's 1 kHz capture thread, ADR 0018): [`Pads::take`] stops
/// the player's polling and its rumble, [`Pads::give_back`] opens them
/// again. A subsystem that takes them gives them back when it goes.
pub struct Pads<'a> {
    gilrs: &'a mut Option<Gilrs>,
    rumble: &'a mut PadRumble,
    /// Whether a subsystem holds them: the player then skips its polling.
    lent: &'a mut bool,
}

impl<'a> Pads<'a> {
    /// The host's pads: its backend, its rumble and its flag for a
    /// subsystem holding them.
    pub fn new(
        gilrs: &'a mut Option<Gilrs>,
        rumble: &'a mut PadRumble,
        lent: &'a mut bool,
    ) -> Self {
        Self {
            gilrs,
            rumble,
            lent,
        }
    }

    /// The pads are the subsystem's until it gives them back: the player's
    /// polling stops and a script's rumble does nothing.
    pub fn take(&mut self) {
        self.rumble.stop_all();
        *self.gilrs = None;
        *self.lent = true;
    }

    /// The player polls the pads again, through a fresh backend.
    pub fn give_back(&mut self) {
        *self.gilrs = Gilrs::new().ok();
        *self.lent = false;
    }
}

/// What a subsystem may touch at a stage of the frame, and at a script
/// command, a key or a click.
pub struct StageCtx<'a> {
    pub world: &'a mut FlintWorld,
    pub clock: &'a GameClock,
    /// The frame's input, which a subsystem may also write (the rhythm
    /// session's pad drain, at [`Stage::Clock`](crate::Stage::Clock)).
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
    pub project: &'a Project,
    pub scene_path: &'a str,
    /// Whether the scene file may be written (#364).
    pub scene_file_writable: bool,
    /// egui's points per pixel, for a pointer in window pixels.
    pub pixels_per_point: f32,
    pub cursor_captured: bool,
    /// What a debug key says when it has nothing to act on (#367): on
    /// screen for a moment and in the console.
    pub note: &'a mut Note,
    /// What scripts asked of the frame, before the host applies it: a
    /// subsystem may fill what they left (the rhythm ladder's post values,
    /// at [`Stage::MadeAssets`](crate::Stage::MadeAssets), before the renderer's
    /// sync).
    pub requests: &'a mut FrameRequests,
    /// The sound, when the host has it.
    pub audio: Option<&'a mut AudioEngine>,
    /// The pads, when the host has them.
    pub pads: Option<Pads<'a>>,
    /// Set to end the run as closing the window would (`quit_on_finish`);
    /// `None` for a host with no run to end.
    pub quit: Option<&'a mut bool>,
    /// The run's resources: what one package lends another.
    pub resources: &'a mut Resources,
    /// The frame's event bus: what a subsystem raises here (a tween's
    /// `tween_done`) reaches the scripts with the next frame's events, as
    /// the physics contacts do.
    pub bus: &'a mut EventBus,
}

impl StageCtx<'_> {
    /// Whether the game state lets the system `name` run this frame (a
    /// subsystem asks by its own `name()`). Outside a stage, it does.
    pub fn runs(&self, name: &str) -> bool {
        self.policy.is_none_or(|policy| policy.runs(name))
    }
}

/// What a subsystem may touch inside a fixed physics step (ADR 0137 section
/// 3a, #1551): [`Subsystem::fixed_update`](crate::Subsystem::fixed_update),
/// after the character controller and the physics sync-in, before Rapier's
/// step. What it applies to a body here, that step integrates.
///
/// A step is not a frame: it lends no scripts, renderer, camera or sound. A
/// package hands its step's results to scripts and the screen at the
/// frame's stages (vehicle state at `PreScripts`, wheel poses at `Physics`).
pub struct FixedCtx<'a> {
    pub world: &'a mut FlintWorld,
    /// Bodies by entity ([`PhysicsSystem::body_of`]), Rapier's sets
    /// ([`PhysicsSystem::rapier_mut`]); Rapier itself is
    /// [`physics::rapier`](crate::physics::rapier).
    pub physics: &'a mut PhysicsSystem,
    /// The step's length, in seconds (the clock's fixed timestep).
    pub dt: f64,
    /// This step's index in the frame (from 0) and the steps the frame
    /// runs: `(1, 2)` is the second of two at 30 Hz.
    pub step: (u32, u32),
    /// Held actions, as the character controller reads them.
    pub input: &'a InputState,
    /// Events a step raises; its contacts go here after Rapier's step.
    pub events: &'a mut EventBus,
    /// Which systems the game state lets run. A step runs only while it
    /// lets physics run; a subsystem with its own policy name asks
    /// [`FixedCtx::runs`].
    pub policy: &'a StateConfig,
    /// The run's resources: what one package lends another.
    pub resources: &'a mut Resources,
}

impl FixedCtx<'_> {
    /// Whether the game state lets the system `name` run (a subsystem asks
    /// by its own `name()`).
    pub fn runs(&self, name: &str) -> bool {
        self.policy.runs(name)
    }
}

/// A capture's own animation (`flint render --anim-time`, #307): the
/// clips, bone probes and IK a package steps on the capture's clock, which
/// the command makes and hands to the capture's game. The capture steps it
/// where the player's frame runs `Stage::Animate` and `Stage::Pose` (which
/// a capture doesn't run), after the scripts and before the bones go up.
pub trait CapturePose: std::any::Any {
    /// One frame of animation, `dt` seconds on.
    fn step(&mut self, world: &mut FlintWorld, dt: f64);

    /// Itself, for the command that made it to read back.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// A mouse button, as [`PlayerInput::Click`] carries it: the kernel's own,
/// so winit is not in the API (ADR 0137 section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Input a subsystem may take before the game and the player see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerInput {
    /// A mouse button, real or scripted, before egui's own use of it:
    /// `pressed` on its press, not on its release.
    Click { button: Button, pressed: bool },
    /// A real Escape, before it releases the cursor or quits.
    Escape,
}

/// What a subsystem may touch after the frame's scene is encoded and before
/// the HUD: a second look at the scene drawn over part of the frame (the
/// Living Props A/B split, #238).
pub struct EncodeCtx<'a> {
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
    /// The run's resources: what one package lends another.
    pub resources: &'a mut Resources,
}

/// What a subsystem may read for the `--exit-after` report.
pub struct ReportCtx<'a> {
    pub renderer: Option<&'a SceneRenderer>,
    /// The run's resources: what one package lends another.
    pub resources: &'a Resources,
}

/// What a subsystem reads when it adds to the frame's inputs.
pub struct FrameCtx<'a> {
    /// The frame's camera, final by the renderer's sync.
    pub camera: &'a Camera,
    /// The shading boil's cadence the config asks for (`[post_process]
    /// boil_on`).
    pub boil_on: u32,
}

/// Lines a subsystem adds to the `--exit-after` report, printed after the
/// player's own.
#[derive(Debug, Default)]
pub struct ExitReport {
    pub lines: Vec<String>,
}

impl ExitReport {
    pub fn line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }
}

/// A capture's game as a package's CLI side reads it (#1558): the scene's
/// subsystems, the run's resources and the world. `flint render`'s
/// `HeadlessGame` lends it, so the render's registration points
/// (`cli::render::StagePart`) reach a package's own subsystem without the
/// command naming it: the package finds it with [`find`].
pub struct CaptureGame<'a> {
    pub subsystems: &'a [Box<dyn Subsystem>],
    pub resources: &'a Resources,
    pub world: &'a FlintWorld,
}

/// [`CaptureGame`], to change: a package's subsystem and resources as a
/// capture draws (living's blends, uploaded each capture).
pub struct CaptureGameMut<'a> {
    pub subsystems: &'a mut [Box<dyn Subsystem>],
    pub resources: &'a mut Resources,
}

/// The subsystem of type `T` among `subsystems`, if there is one.
pub fn find<T: Subsystem>(subsystems: &[Box<dyn Subsystem>]) -> Option<&T> {
    subsystems
        .iter()
        .find_map(|s| (s.as_ref() as &dyn std::any::Any).downcast_ref::<T>())
}

/// The subsystem of type `T` among `subsystems`, to change.
pub fn find_mut<T: Subsystem>(subsystems: &mut [Box<dyn Subsystem>]) -> Option<&mut T> {
    subsystems
        .iter_mut()
        .find_map(|s| (s.as_mut() as &mut dyn std::any::Any).downcast_mut::<T>())
}

/// How long a subsystem lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lasts {
    /// The whole run: the host made it (the expression feed).
    Run,
    /// One scene: built by its builder when a scene that uses it is
    /// entered, dropped when that scene unloads.
    Scene,
}
