//! A scene's sound (#1619): the player's audio calls, each at the stage of
//! the frame where the player made it before the package was one. With no
//! sound opened (a capture, a test core) every hook does nothing, and the
//! script commands go on to the host as before.

use super::{loading, system_of_mut, POLICY};
use crate::engine::{KeyedPlay, DEFAULT_DISTANCES, DEFAULT_ROLLOFF};
use crate::AudioSystem;
use flint_core::frame_notes::{self, NoteKind};
use flint_core::Vec3;
use flint_package::{SceneCtx, Stage, StageCtx, Subsystem};
use flint_runtime::{PlayerView, RuntimeSystem};
use flint_script::context::{ScriptCommand, SoundPlay, SoundRef};
use std::time::Duration;

/// Frame time given to decoding sounds while a late audio bundle settles in
/// (#124). A song is 85 to 190 ms of decoding in Chrome; at 2 ms a frame it
/// spreads over a second or two, and Float's 10 ms frames stay inside one
/// 16.7 ms vsync (at 4 ms they slipped to 33 ms now and then).
const DECODE_BUDGET: Duration = Duration::from_millis(2);

/// The scene's sound: built for every scene.
pub struct AudioSubsystem {
    /// The scene's `[scene] preload_audio`, for a late bundle's sounds.
    preload: bool,
    /// The file source's generation the scene's sounds were loaded at: a
    /// late bundle (audio, #98) changes it.
    generation: u64,
}

/// Every scene's: its sounds load as it is built, before its scripts'
/// `on_init`, as the player loaded them.
pub(super) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    // Before the load, so a bundle landing meanwhile is loaded next frame
    let generation = flint_core::vfs::generation();
    // The player's `--music-volume` / `--sfx-volume`, once a run, over the
    // game's buses: each scales the volume its bus starts at
    let scales = ctx.resources.remove::<super::VolumeArgs>();
    if let Some(system) = system_of_mut(ctx.resources) {
        if let Some(scales) = scales {
            for (bus, scale) in [
                (crate::Bus::MUSIC, scales.music_volume),
                (crate::Bus::SFX, scales.sfx_volume),
            ] {
                let declared = system.engine.bus_volume(bus).unwrap_or(1.0);
                system.set_bus_volume(bus, declared * scale);
            }
        }
        let _note = frame_notes::begin(NoteKind::AudioLoad, String::new);
        loading::load_audio_from_world(
            ctx.world,
            system,
            ctx.project,
            ctx.scene_path,
            ctx.preload_audio,
            // In the browser the audio may already be here, and decoding it
            // all in one frame is the stall #124 removed: queue it. The
            // desktop decodes up front, as a load can afford.
            cfg!(target_arch = "wasm32"),
        );
        system
            .initialize(ctx.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));
    }
    Some(Box::new(AudioSubsystem {
        preload: ctx.preload_audio,
        generation,
    }))
}

/// A script's sound as the engine plays it, on the bus it names: any the
/// game declares (#730); an unknown one is warned about once and plays on
/// sfx.
fn keyed(system: &mut AudioSystem, play: &SoundPlay) -> KeyedPlay {
    KeyedPlay {
        volume: play.volume,
        pitch: play.pitch,
        looping: play.looping,
        bus: system.engine.bus_or_sfx(&play.bus),
    }
}

impl AudioSubsystem {
    /// The listener to `PlayerView::listener`, when the kernel moved it.
    fn follow_listener(&self, stage: Stage, ctx: &mut StageCtx) {
        let Some(view) = ctx.resources.get::<PlayerView>() else {
            return;
        };
        let (listener, eye) = (view.listener, view.eye);
        let to = match stage {
            // The first-person eye, while the sound runs
            Stage::Physics => listener.filter(|_| eye.is_some() && ctx.runs(POLICY)),
            // A script-driven camera's (chase cam, etc.), where no
            // controller placed an eye (and so the kernel set none at
            // `Physics`), whatever the policy, as before
            _ => listener.filter(|_| eye.is_none()),
        };
        if let (Some(at), Some(system)) = (to, system_of_mut(ctx.resources)) {
            system.update_listener(at.position, at.yaw, at.pitch);
        }
    }

    /// The frame's sound, before animation moves on.
    fn sound(&mut self, ctx: &mut StageCtx) {
        let runs = ctx.runs(POLICY);
        let Some(system) = system_of_mut(ctx.resources) else {
            return;
        };
        flint_core::scope!("sound");
        // A late bundle (audio, #98) landed: queue what wasn't there at
        // start. Already-loaded sounds are skipped, and an autoplay source
        // that was waiting for its file starts on the update after its
        // sound is decoded.
        let generation = flint_core::vfs::generation();
        if generation != self.generation {
            self.generation = generation;
            loading::load_audio_from_world(
                ctx.world,
                system,
                ctx.project,
                ctx.scene_path,
                self.preload,
                true,
            );
        }
        // ...and decode it a few milliseconds a frame (#124).
        system.engine.decode_queued(DECODE_BUDGET);

        // Audio triggers from game events (skip when paused)
        if runs {
            system.process_events(ctx.events, ctx.world);
            system.update(ctx.world, ctx.clock.delta_time).ok();
        }
    }
}

impl Subsystem for AudioSubsystem {
    fn name(&self) -> &'static str {
        POLICY
    }

    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        // Last of the scene's subsystems to hear it: the music's stems
        // have stopped by now (ADR 0017's producer→handles→device order)
        if let Some(system) = system_of_mut(ctx.resources) {
            system.clear();
        }
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            // Where each audio_source is in its sound (#658), read every
            // frame, paused or not: loops play on under a pause menu. And
            // which of the sounds scripts started still play (#1000): a
            // command from last frame's batch is in the engine by now
            Stage::PreScripts => {
                if let Some(system) = system_of_mut(ctx.resources) {
                    system.sample_positions();
                    ctx.script
                        .set_audio_clock(system.sync.positions(), system.output_latency());
                    ctx.script.set_sounds_playing(system.engine.playing_keys());
                }
            }
            Stage::Physics | Stage::PostPhysics => self.follow_listener(stage, ctx),
            Stage::Sound => self.sound(ctx),
            // A script's master low-pass, once a frame
            Stage::Sync => {
                if let Some(system) = system_of_mut(ctx.resources) {
                    if let Some(cutoff) = ctx.requests.audio_lowpass.take() {
                        system.set_filter_cutoff(cutoff);
                    }
                }
            }
            _ => {}
        }
    }

    // A script's sounds are the engine's keyed ones (#699): the handle
    // `play_sound` returned is the key `stop_sound` and `set_sound_volume`
    // find it by, on kira here or in the browser's worklet alike. A
    // player's own volume (`set_user_bus_volume`) is the host's: it keeps
    // them in its settings file.
    fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        use ScriptCommand as C;
        if !matches!(
            command,
            C::PlaySound { .. }
                | C::PlaySoundAt { .. }
                | C::StopSound { .. }
                | C::SetSoundVolume { .. }
                | C::SetBusVolume { .. }
                | C::SetBusLowpass { .. }
        ) {
            return false;
        }
        let Some(system) = system_of_mut(ctx.resources) else {
            return false;
        };
        match command {
            C::PlaySound { handle, name, play } => {
                if system.engine.is_available() {
                    let play = keyed(system, play);
                    if let Err(e) = system.engine.play_keyed(*handle, name, play) {
                        tracing::warn!(target: "script", "play_sound error: {:?}", e);
                    }
                }
            }
            C::PlaySoundAt {
                handle,
                name,
                position,
                play,
                distances,
                rolloff,
            } => {
                // As `play_sound`: with no device there's nothing to say
                // (#714 found it warning on every call in a silent run)
                if system.engine.is_available() {
                    let pos = Vec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
                    let play = keyed(system, play);
                    if let Err(e) = system.engine.play_keyed_at(
                        *handle,
                        name,
                        pos,
                        distances.unwrap_or(DEFAULT_DISTANCES),
                        rolloff.unwrap_or(DEFAULT_ROLLOFF),
                        play,
                    ) {
                        tracing::warn!(target: "script", "play_sound_at error: {:?}", e);
                    }
                }
            }
            C::StopSound { sound, fade } => {
                let fade = Duration::from_secs_f64(fade.max(0.0));
                match sound {
                    SoundRef::Handle(h) => {
                        system.engine.stop_keyed(*h, fade);
                    }
                    SoundRef::Name(name) => {
                        system.engine.stop_keyed_named(name, fade);
                    }
                }
            }
            C::SetSoundVolume {
                handle,
                volume,
                fade,
            } => {
                let fade = Duration::from_secs_f64(fade.max(0.0));
                system.engine.set_keyed_volume(*handle, *volume, fade);
            }
            // Kept while silent too, so the mixer opens at it (the web's
            // Begin). A name that is no bus is warned about once, by the
            // engine.
            C::SetBusVolume { bus, volume } => {
                let _ = system.engine.set_bus_volume_named(bus, *volume);
            }
            C::SetBusLowpass { bus, hz } => {
                let _ = system.engine.set_bus_lowpass_named(bus, *hz);
            }
            _ => unreachable!(),
        }
        true
    }
}
