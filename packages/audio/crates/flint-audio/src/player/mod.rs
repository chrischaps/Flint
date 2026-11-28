//! The audio package's player part (#1619, ADR 0138): what a launcher
//! lists ([`PACKAGE`]). The kernel names none of it.
//!
//! - **The sound** ([`AudioSystem`]) is the run's resource, as the host's
//!   [`flint_package::sound::Sound`]: a host opens it through
//!   [`SoundPoint`] (with a device, or silent), and it lasts the run, its
//!   decoded sounds with it. Music and rhythm play on its engine
//!   ([`crate::engine::of_mut`]).
//! - **A scene's sound** is [`AudioSubsystem`], built for every scene: the
//!   scene's sounds load as it is built, and the frame is where the player
//!   ran it before the package was one:
//!
//! | when | what |
//! |---|---|
//! | the scene's build | the scene's sounds (each `audio_source`'s, then `audio/`), the listener, the trigger rules, the sources |
//! | `Stage::PreScripts` | where each source is in its sound, the latency and the playing handles, for the scripts |
//! | `Stage::Physics` | the listener to the eye a controller placed (`PlayerView::listener`), while the `audio` policy runs |
//! | `Stage::PostPhysics` | the listener to the script's camera, where no controller placed an eye |
//! | `Stage::Sound` | a late bundle's sounds, the decode queue, then (while the policy runs) the triggers from the frame's events and the sources |
//! | `Stage::Sync` | a script's master low-pass (`set_audio_lowpass`) |
//! | `script_command` | the sounds scripts play, stop and fade, and the buses they set |
//! | the scene's end | one-shots and script loops stop; the sources forget it |
//!
//! - **The `audio` script module** ([`module::AUDIO_MODULE`]) is on in
//!   every scene of a run with the package.
//! - **A game's extension** plays on it through [`audio`].

pub mod extension;
pub mod loading;
pub mod module;
mod sound;
mod subsystem;

use crate::AudioSystem;
use flint_core::components as comp;
use flint_package::sound::SoundPoint;
use flint_package::{Package, Registry, Resources, SceneCtx};

pub use extension::{audio, ExtensionAudio, SoundOptions};
pub use subsystem::AudioSubsystem;

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "audio";

/// The name a game state's policy pauses the sound by: triggers, the
/// sources' update and the first-person listener.
pub const POLICY: &str = "audio";

/// The `audio` package (`packages/audio/package.toml`).
pub static PACKAGE: AudioPackage = AudioPackage;

pub struct AudioPackage;

impl Package for AudioPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its builder (every scene has one), its policy name, its line for a
    /// project that leaves it out, the `audio` module, the volume flags,
    /// and how a host opens the sound.
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(subsystem::for_scene)
            .policy(POLICY)
            .when_left_out(left_out)
            .every_scene_modules(&[&module::AUDIO_MODULE])
            .player_args::<VolumeArgs>()
            .insert_point(SoundPoint {
                open: sound::open,
                unlock: sound::unlock,
                page: sound::page,
                check_buses: |project| {
                    crate::Buses::for_project(project)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
            });
    }
}

/// The player's flags for the sound's starting volumes (they were the
/// kernel's `PlayerArgs` until #1619).
#[derive(Debug, Clone, clap::Args)]
pub struct VolumeArgs {
    /// Scales the music bus's starting volume (linear, 0.0 = muted, 1.0 = the
    /// volume the game's flint.toml gives it, or full)
    #[arg(long, default_value_t = 1.0)]
    pub music_volume: f64,

    /// Scales the SFX bus's starting volume (linear, 0.0 = muted, 1.0 = the
    /// volume the game's flint.toml gives it, or full)
    #[arg(long, default_value_t = 1.0)]
    pub sfx_volume: f64,
}

/// The run's sound, if a host opened one (the player, a playtest): the
/// audio package's [`AudioSystem`].
pub fn system_of(resources: &Resources) -> Option<&AudioSystem> {
    flint_package::sound::of(resources)?
        .as_any()
        .downcast_ref::<AudioSystem>()
}

/// [`system_of`], to change.
pub fn system_of_mut(resources: &mut Resources) -> Option<&mut AudioSystem> {
    flint_package::sound::of_mut(resources)?
        .as_any_mut()
        .downcast_mut::<AudioSystem>()
}

/// What a scene with sounds hears when the project's `[packages]` leaves
/// `audio` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::AUDIO_SOURCE, comp::AUDIO_TRIGGER]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "stay silent")
}
