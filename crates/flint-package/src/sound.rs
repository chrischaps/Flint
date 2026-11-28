//! The run's sound as a host sees it (ADR 0138 decision 1, #1619).
//!
//! The audio package owns the sound: its mixer is the run's resource, and
//! its subsystem plays a scene's sources, triggers and script sounds at the
//! frame's stages. A host still has a few things to ask of it that are
//! the host's to decide, and this is all of them, so the host names no
//! audio crate:
//!
//! - **Opening it** ([`SoundPoint::open`]): with a device, or silent (a
//!   headless playtest). A host that never opens one (a capture, a test
//!   core) runs with none, and every hook of the package does nothing.
//! - **The page's gesture** ([`Sound::start`], [`SoundPoint::unlock`]): a
//!   browser gives no sound before one (#101).
//! - **The page's sleep and hold** ([`SoundPoint::page`], [`Sound::hold`],
//!   #162): a hidden page's sound sleeps, and the game holds with it.
//! - **The game's buses** ([`Sound::declare_buses`], #730), from the
//!   project's `flint.toml`, when the project is read.
//! - **The player's own volumes** ([`Sound::user_volumes`] and the rest,
//!   #1217): the host keeps them in its settings file.
//! - **The perf readout** ([`Sound::meter`], #138) and the web's old mix
//!   (`?mix=old`, [`Sound::set_old_mix`]).
//!
//! A package that plays on the sound itself (music, rhythm) names the audio
//! package's crate and asks for its engine by type
//! (`flint_audio::engine::of_mut`).

use crate::Resources;
use flint_core::project::Project;
use flint_core::sound::MixCounts;
use flint_ecs::FlintWorld;
use std::any::Any;
use std::collections::BTreeMap;

/// What a host asks the sound to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// The sound card; in the browser, the AudioWorklet once the page's
    /// gesture lets it start ([`Sound::start`]).
    Device,
    /// No device, nothing heard and nothing said of it: sounds still load,
    /// so a missing file still warns (a headless playtest, #714).
    Silent,
}

/// The sound's lines in the perf readout.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    /// Late audio buffers of all buffers so far, one buffer's length and
    /// the mean time to mix one, ms; `None` before the sound starts, and on
    /// the desktop, whose device stream isn't metered.
    pub buffers: Option<(u64, u64, f64, f64)>,
    /// The voices and spatial tracks being mixed (#138); `None` without a
    /// device.
    pub mix: Option<MixCounts>,
}

/// The run's sound, as the host asks it things: the audio package's
/// resource, `Box<dyn Sound>` in the run's [`Resources`] once a host has
/// opened it ([`of_mut`]).
pub trait Sound: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Begin sound, from the user gesture that begins the game (#101):
    /// the device opens if it hasn't, and the scene's loops start. Says
    /// whether there is sound.
    fn start(&mut self, world: &FlintWorld) -> bool;

    /// The mixer's buses from `project`'s `flint.toml` (#730). Applied once
    /// a table, so asking at every scene of one project keeps the volumes
    /// set since. A manifest that doesn't read is said.
    fn declare_buses(&mut self, project: &Project);

    /// The page holds the game (#162): the mix fades out, and back in when
    /// it lets go.
    fn hold(&mut self, held: bool);

    /// What the perf readout says of the sound.
    fn meter(&self) -> Meter;

    /// Mix as before #138, to compare against (the web's `?mix=old`). Set
    /// it before the scene's sounds start.
    fn set_old_mix(&mut self, old: bool);

    /// Whether [`set_old_mix`](Self::set_old_mix) asked for the old mix.
    fn old_mix(&self) -> bool;

    /// The player's own bus volumes, by bus name (#1217).
    fn user_volumes(&self) -> BTreeMap<String, f64>;

    /// The player's volumes, as the host read them from its settings.
    fn load_user_volumes(&mut self, volumes: BTreeMap<String, f64>);

    /// One of the player's volumes (a script's `set_user_bus_volume`).
    /// Says whether it changed: a bus that isn't one, or the volume it
    /// already had, didn't.
    fn set_user_volume(&mut self, bus: &str, volume: f64) -> bool;
}

/// The audio package's registration point (`Registry::point::<SoundPoint>()`):
/// how a host opens the sound, and the browser's page hooks that run
/// before there is a run to ask (the Begin click's handler).
#[derive(Clone, Copy)]
pub struct SoundPoint {
    /// The run's sound, for the host to put in its resources ([`open`]).
    pub open: fn(Output) -> Box<dyn Sound>,
    /// The page's Begin gesture (the web): the `AudioContext` is opened
    /// inside it, since iOS starts one only there. Nothing elsewhere.
    pub unlock: fn(),
    /// Whether the page is hidden (its sound asleep), and how many times it
    /// has been: a host that saw another count last frame slept in between
    /// (#162). `(false, 0)` off the web.
    pub page: fn() -> (bool, u64),
    /// Read `project`'s `flint.toml` `[audio.buses]` (#730) as the player
    /// would at start: `flint check`'s `bad-manifest` when it doesn't read.
    pub check_buses: fn(&Project) -> Result<(), String>,
}

/// The run's sound, if a host opened one.
pub fn of(resources: &Resources) -> Option<&dyn Sound> {
    resources.get::<Box<dyn Sound>>().map(|s| s.as_ref())
}

/// The run's sound, to change, if a host opened one.
pub fn of_mut(resources: &mut Resources) -> Option<&mut dyn Sound> {
    resources.get_mut::<Box<dyn Sound>>().map(|s| s.as_mut())
}

/// Open the sound through `point` and put it in `resources`, replacing one
/// opened before.
pub fn open(point: &SoundPoint, resources: &mut Resources, output: Output) {
    resources.insert((point.open)(output));
}
