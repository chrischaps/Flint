//! kira somewhere else (#136, #730): the browser's audio thread, reached
//! through a [`Link`]. Every call is a [`Command`]; each sound crosses once,
//! as samples.

use super::backend::{Held, MixerBackend, On, Unhosted, Voice};
use super::{AudioBackend, MixCounts};
use crate::bus::Bus;
use crate::handle::{SoundHandle, SpatialTrack};
use crate::pan::Falloff;
use crate::remote::{protocol, Command, Link, RemoteTrack, RemoteVoice, SoundLoad, Target};
use flint_core::{FlintError, Result};
use kira::sound::static_sound::StaticSoundData;
use kira::{AudioManager, Decibels};
use std::collections::HashMap;
use std::time::Duration;
use web_time::Instant;

pub(crate) struct Remote {
    pub(crate) link: Link,
    /// Every sound sent, by name, and how long each plays, seconds: the
    /// mixer reports only watched voices (#658), so a one-shot's end is
    /// worked out here.
    sounds: HashMap<String, u32>,
    lengths: HashMap<u32, f64>,
    listener: bool,
    /// The worklet behind the link, while it starts and once it runs.
    #[cfg(target_arch = "wasm32")]
    pub(crate) stream: Option<crate::worklet::WorkletStream>,
}

impl Remote {
    pub(crate) fn new(link: Link) -> Self {
        Self {
            link,
            sounds: HashMap::new(),
            lengths: HashMap::new(),
            listener: false,
            #[cfg(target_arch = "wasm32")]
            stream: None,
        }
    }

    fn sound(&self, name: &str) -> Result<u32> {
        self.sounds
            .get(name)
            .copied()
            .ok_or_else(|| FlintError::AudioError(format!("Sound not cached: {name}")))
    }

    fn no_listener() -> FlintError {
        FlintError::AudioError("No listener created".into())
    }
}

impl MixerBackend for Remote {
    fn name(&self) -> &'static str {
        "worklet"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn is_remote(&self) -> bool {
        true
    }
    fn failed(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        if let Some(s) = &self.stream {
            return s.failed();
        }
        false
    }

    /// Until the worklet is known to run, the main-thread fallback may
    /// need the sounds.
    fn needs_cache(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        if let Some(s) = &self.stream {
            return !s.running();
        }
        false
    }

    /// Send a sound, under the id it already has if it was sent before.
    fn sound_cached(&mut self, name: &str, data: &StaticSoundData) {
        let id = match self.sounds.get(name) {
            Some(&id) => id,
            None => {
                let id = self.link.id();
                self.sounds.insert(name.to_string(), id);
                id
            }
        };
        self.lengths.insert(
            id,
            data.frames.len() as f64 / data.sample_rate.max(1) as f64,
        );
        self.link
            .load(SoundLoad::new(id, data.sample_rate, &data.frames));
    }

    fn holds_sound(&self, name: &str) -> bool {
        self.sounds.contains_key(name)
    }

    fn sound_names(&self) -> Vec<&str> {
        self.sounds.keys().map(String::as_str).collect()
    }

    fn sound_length(&self, name: &str) -> Option<f64> {
        self.lengths.get(self.sounds.get(name)?).copied()
    }

    fn add_bus(&mut self, bus: Bus, parent: Option<Bus>, db: Decibels) -> Result<()> {
        self.link.send(Command::BusNew {
            bus: bus.code(),
            parent: parent.map_or(protocol::BUS_MAIN, Bus::code),
            db: db.0,
        });
        Ok(())
    }

    fn set_bus_volume(&mut self, bus: Bus, db: Decibels, tween_secs: f64) {
        self.link.send(Command::BusVolume {
            bus: bus.code(),
            db: db.0,
            tween: tween_secs as f32,
        });
    }

    fn set_filter_cutoff(&mut self, hz: f32, tween_secs: f64) {
        self.link.send(Command::FilterCutoff {
            hz,
            tween: tween_secs as f32,
        });
    }

    fn create_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat) -> Result<()> {
        self.link.send(Command::Listener {
            pos: pos.to_array(),
            quat: orientation.to_array(),
        });
        self.listener = true;
        Ok(())
    }

    fn has_listener(&self) -> bool {
        self.listener
    }

    fn move_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat) {
        if self.listener {
            self.link.send(Command::Listener {
                pos: pos.to_array(),
                quat: orientation.to_array(),
            });
        }
    }

    fn create_spatial_track(
        &mut self,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<SpatialTrack> {
        if !self.listener {
            return Err(Self::no_listener());
        }
        let id = self.link.id();
        self.link.send(Command::TrackNew {
            track: id,
            pos: pos.to_array(),
            min: falloff.min,
            max: falloff.max,
            rolloff: falloff.rolloff,
            bus: bus.code(),
        });
        Ok(SpatialTrack::Remote(RemoteTrack {
            id,
            link: self.link.clone(),
        }))
    }

    fn play(&mut self, voice: Voice<'_>, on: On<'_>) -> Result<SoundHandle> {
        let target = match on {
            On::Bus(bus) => Target::Bus(bus.code()),
            On::Track(SpatialTrack::Remote(t)) => Target::Track(t.id),
            On::Track(SpatialTrack::Local(_)) => {
                return Err(FlintError::AudioError(
                    "the mix moved; this track is gone".into(),
                ))
            }
        };
        let sound = self.sound(voice.name)?;
        let id = self.link.id();
        self.link.send(Command::Play {
            voice: id,
            sound,
            target,
            db: voice.db.0,
            rate: voice.rate as f32,
            pan: voice.pan,
            looping: voice.looping,
        });
        Ok(SoundHandle::Remote(RemoteVoice {
            id,
            link: self.link.clone(),
        }))
    }

    /// The worklet keeps the track until the sound ends.
    fn play_on_own_track(
        &mut self,
        voice: Voice<'_>,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<Option<(SoundHandle, SpatialTrack)>> {
        if !self.listener {
            return Err(Self::no_listener());
        }
        self.link.send(Command::OneShot {
            sound: self.sound(voice.name)?,
            pos: pos.to_array(),
            db: voice.db.0,
            rate: voice.rate as f32,
            min: falloff.min,
            max: falloff.max,
            rolloff: falloff.rolloff,
            bus: bus.code(),
        });
        Ok(None)
    }

    fn play_ends(&self, name: &str, rate: f64, looping: bool, now: Instant) -> Option<Instant> {
        if looping {
            return None;
        }
        let secs = self.sound_length(name)? / rate.abs().max(1e-3);
        Some(now + Duration::from_secs_f64(secs))
    }

    fn clear_oneshots(&mut self) {
        self.link.send(Command::ClearOneShots);
    }

    /// The worklet says nothing back: what the engine started.
    fn add_counts(&self, counts: &mut MixCounts, held: &Held) -> Option<()> {
        counts.voices += held.placed + held.keyed;
        Some(())
    }

    /// The browser's `AudioContext` time; `None` for a link whose clock
    /// can't be read (a test's bare link).
    fn clock_now(&self, _: Instant) -> Option<f64> {
        self.link.clock().map(|[now, _]| now)
    }

    fn output_latency(&self) -> Option<f64> {
        self.link.clock().map(|[_, latency]| latency)
    }

    fn host(&mut self) -> std::result::Result<&mut AudioManager<AudioBackend>, Unhosted> {
        Err(Unhosted::Worklet)
    }
}
