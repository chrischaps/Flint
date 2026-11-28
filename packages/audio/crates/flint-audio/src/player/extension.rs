//! The sound as a game's own Rust reaches it (ADR 0084): a
//! `PlayerExtension` asks [`audio`] for it, on the resources its context
//! lends (moved from the player with the package, #1619; it was
//! `ExtensionCtx::audio`).

use crate::{AudioSystem, Bus};
use flint_core::Vec3;
use flint_package::Resources;

/// The run's sound, as an extension plays on it: `None` in a run with no
/// sound opened (a capture, a test core).
pub fn audio(resources: &mut Resources) -> Option<ExtensionAudio<'_>> {
    super::system_of_mut(resources).map(ExtensionAudio::new)
}

/// The player's audio, as an extension reaches it (ADR 0084).
///
/// Sounds are cached by name, the same cache scripts' `play_sound` and
/// `audio_source.file` read: a sound loaded here can be played from a
/// script, and a file the scene preloaded (`audio/<file>`) can be played
/// from here. Loading needs no audio device; playing without one does
/// nothing, so a game needn't check.
///
/// ```ignore
/// let Some(mut audio) = flint_audio::player::audio(ctx.resources) else { return };
/// // Once: a sound made in code, mono at 44.1 kHz.
/// audio.load_samples("blip", 44_100, &[&samples])?;
/// // Any frame:
/// audio.play("blip", &SoundOptions::new().volume(0.5).pitch(1.2))?;
/// audio.play("audio/door.ogg", &SoundOptions::new().at(pos).distances(2.0, 60.0))?;
/// // Heard from far off, fading like a sound in the open:
/// audio.play("bell", &SoundOptions::new().at(pos).distances(10.0, 400.0).rolloff(0.7))?;
/// audio.set_bus_volume(Bus::MUSIC, 0.4);
/// // A bus the game's flint.toml declares ([audio.buses.voice]):
/// if let Some(voice) = audio.bus("voice") {
///     audio.play("line_04", &SoundOptions::new().bus(voice))?;
/// }
/// ```
pub struct ExtensionAudio<'a> {
    system: &'a mut AudioSystem,
}

impl<'a> ExtensionAudio<'a> {
    pub fn new(system: &'a mut AudioSystem) -> Self {
        Self { system }
    }

    /// Whether there's a device to play on. Without one, every sound is
    /// silent, though loading still works.
    pub fn is_available(&self) -> bool {
        self.system.engine.is_available()
    }

    /// Whether a sound is cached under `name`.
    pub fn has_sound(&self, name: &str) -> bool {
        self.system.engine.has_sound(name)
    }

    /// Cache a sound made in memory: one channel (heard on both sides) or
    /// two (left, right) of samples in -1..1 at `sample_rate` Hz. Replaces
    /// any sound cached under `name`; one already playing plays on.
    ///
    /// A looping `audio_source` naming a sound that isn't cached yet stays
    /// silent, so load before spawning the entity that plays it.
    pub fn load_samples(
        &mut self,
        name: &str,
        sample_rate: u32,
        channels: &[&[f32]],
    ) -> flint_core::Result<()> {
        self.system
            .engine
            .load_sound_from_samples(name, sample_rate, channels)
    }

    /// Cache an encoded sound held in memory (WAV, OGG, FLAC or MP3), such
    /// as one from `include_bytes!`. Replaces any sound cached under `name`.
    pub fn load_bytes(&mut self, name: &str, bytes: Vec<u8>) -> flint_core::Result<()> {
        self.system.engine.load_sound_from_bytes(name, bytes)
    }

    /// Play the cached sound `name` once. A preloaded file answers to its
    /// path (`audio/door.ogg`) or its bare stem (`door`), as in scripts.
    /// An unknown name is an error naming the nearest cached sound; with
    /// no device it does nothing.
    pub fn play(&mut self, name: &str, options: &SoundOptions) -> flint_core::Result<()> {
        let engine = &mut self.system.engine;
        let resolved = engine.resolve_sound(name).into_owned();
        let name = resolved.as_str();
        if !engine.has_sound(name) {
            return Err(engine.not_cached(name));
        }
        if !engine.is_available() {
            return Ok(());
        }
        match options.at {
            Some(at) => engine.play_oneshot_at(
                name,
                at,
                options.volume,
                options.pitch,
                options.distances,
                options.rolloff,
                options.bus,
            ),
            None => engine
                .play_non_spatial(name, options.volume, options.pitch, false, options.bus)
                .map(drop),
        }
    }

    /// Set a mixer bus's volume (linear: 0 mutes, 1 is unity). It scales
    /// everything on the bus, scripts' sounds and `audio_source`s included,
    /// on top of their own volumes. It stays set across scene transitions.
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f64) {
        self.system.set_bus_volume(bus, volume);
    }

    /// Set a mixer bus's own low-pass cutoff, Hz (20000 is open, as low as
    /// 10; #1218): it muffles the bus and the buses under it, nothing else.
    pub fn set_bus_lowpass(&mut self, bus: Bus, hz: f64) {
        self.system.set_bus_lowpass(bus, hz);
    }

    /// A bus by name: `sfx`, `music`, or one the game's `flint.toml`
    /// declares under `[audio.buses]` (#730), for [`SoundOptions::bus`] and
    /// [`set_bus_volume`](Self::set_bus_volume).
    pub fn bus(&self, name: &str) -> Option<Bus> {
        self.system.engine.bus(name)
    }

    /// Declare buses in code, as a manifest's `[audio.buses]` would: a game
    /// that builds its mix at run time. Buses already there keep their
    /// numbers; see `AudioEngine::set_buses`.
    pub fn set_buses(&mut self, buses: &crate::Buses) {
        self.system.engine.set_buses(buses);
    }
}

/// How [`ExtensionAudio::play`] plays a sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundOptions {
    /// Linear: 1 is the sound as made.
    pub volume: f64,
    /// Playback rate: 2 is an octave up and twice as fast.
    pub pitch: f64,
    pub bus: Bus,
    /// Where the sound is, world units, so it pans and fades with distance
    /// from the listener. `None` plays it flat, from nowhere.
    pub at: Option<Vec3>,
    /// For a placed sound, `(min, max)` from the listener: full volume
    /// within `min`, silent past `max`.
    pub distances: (f32, f32),
    /// The curve between them (see [`crate::engine::DEFAULT_ROLLOFF`]):
    /// 2 holds the sound up and drops it near `max`; below 1 it falls away
    /// soon after `min` and tails off, like a real sound in the open.
    pub rolloff: f32,
}

impl Default for SoundOptions {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            bus: Bus::SFX,
            at: None,
            distances: crate::engine::DEFAULT_DISTANCES,
            rolloff: crate::engine::DEFAULT_ROLLOFF,
        }
    }
}

impl SoundOptions {
    /// Full volume, normal pitch, on the sfx bus, from nowhere.
    pub fn new() -> Self {
        Self::default()
    }
    pub fn volume(self, volume: f64) -> Self {
        Self { volume, ..self }
    }
    pub fn pitch(self, pitch: f64) -> Self {
        Self { pitch, ..self }
    }
    pub fn bus(self, bus: Bus) -> Self {
        Self { bus, ..self }
    }
    /// Place the sound at `at`.
    pub fn at(self, at: Vec3) -> Self {
        Self {
            at: Some(at),
            ..self
        }
    }
    /// The falloff of a placed sound (default 1 to 25).
    pub fn distances(self, min: f32, max: f32) -> Self {
        Self {
            distances: (min, max),
            ..self
        }
    }
    /// The falloff curve of a placed sound (default 2).
    pub fn rolloff(self, rolloff: f32) -> Self {
        Self { rolloff, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_ecs::FlintWorld;

    #[test]
    fn an_extension_loads_and_plays_sounds_made_in_memory() {
        use flint_runtime::RuntimeSystem;
        // As the player does at scene load: the listener placed sounds need.
        let mut system = AudioSystem::new();
        system.initialize(&mut FlintWorld::new()).unwrap();
        let mut audio = ExtensionAudio::new(&mut system);
        let click: Vec<f32> = (0..200).map(|i| 1.0 - i as f32 / 200.0).collect();
        audio.load_samples("click", 44_100, &[&click]).unwrap();
        assert!(audio.has_sound("click"));
        // Flat and placed, with or without a device.
        audio
            .play("click", &SoundOptions::new().volume(0.5).pitch(1.3))
            .unwrap();
        let placed = SoundOptions::new()
            .at(Vec3::new(4.0, 0.0, 2.0))
            .distances(2.0, 80.0)
            .rolloff(0.7)
            .bus(Bus::MUSIC);
        assert_eq!(placed.distances, (2.0, 80.0));
        assert_eq!(placed.rolloff, 0.7);
        assert_eq!(SoundOptions::new().rolloff, 2.0);
        audio.play("click", &placed).unwrap();
        assert!(audio.play("missing", &SoundOptions::new()).is_err());
        audio.set_bus_volume(Bus::SFX, 0.25);
        audio.set_bus_lowpass(Bus::SFX, 800.0);
        assert_eq!(system.engine.bus_lowpass(Bus::SFX), Some(800.0));
        // Scripts and audio_sources read the same cache.
        assert!(system.engine.has_sound("click"));
    }
}
