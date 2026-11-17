//! kira on this thread (#136, #730): the desktop's mix, and the browser's
//! when its worklet can't run.

use super::backend::{Held, MixerBackend, On, Unhosted, Voice};
use super::{tween_ms, AudioBackend, MixCounts};
use crate::bus::Bus;
use crate::handle::{SoundHandle, SpatialTrack};
use crate::pan::Falloff;
use flint_core::{FlintError, Result};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::StaticSoundData;
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, TrackBuilder, TrackHandle};
use kira::{AudioManager, Decibels, Easing};
use web_time::Instant;

/// kira, here: its manager, the master filter and the buses, by number.
pub(crate) struct Local {
    manager: AudioManager<AudioBackend>,
    filter: FilterHandle,
    /// Every bus, by number (`None`: kira refused to make it, and what
    /// would play on it says so). The built-in two hang off the main track,
    /// and so does every other bus without a parent.
    buses: Vec<Option<TrackHandle>>,
    /// How many buses hang off another bus: kira counts those among the
    /// parent's sub-tracks, beside the spatial tracks.
    child_buses: usize,
    listener: Option<kira::listener::ListenerHandle>,
}

impl Local {
    /// Open kira on [`AudioBackend`] with the main track's low-pass at
    /// `cutoff` and the two built-in buses; `None` with no device.
    #[cfg(any(feature = "device", target_arch = "wasm32"))]
    pub(crate) fn open(
        mut settings: kira::AudioManagerSettings<AudioBackend>,
        cutoff: f64,
    ) -> Option<Self> {
        let filter = settings.main_track_builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(cutoff),
        );
        let mut manager = AudioManager::<AudioBackend>::new(settings)
            .map_err(|e| tracing::warn!("No audio device available ({e}), running silent"))
            .ok()?;
        let buses = Bus::BUILT_IN
            .iter()
            .map(|name| {
                manager
                    .add_sub_track(TrackBuilder::new())
                    .map_err(|e| tracing::warn!("Failed to create the {name} bus: {e}"))
                    .ok()
            })
            .collect();
        Some(Self {
            manager,
            filter,
            buses,
            child_buses: 0,
            listener: None,
        })
    }

    fn bus(&mut self, bus: Bus) -> Result<&mut TrackHandle> {
        self.buses
            .get_mut(bus.index())
            .and_then(Option::as_mut)
            .ok_or_else(|| FlintError::AudioError(format!("bus {} unavailable", bus.index())))
    }

    /// A cached sound with its volume, rate, panning and loop set.
    fn data(voice: &Voice<'_>) -> Result<StaticSoundData> {
        let mut data = voice
            .data
            .ok_or_else(|| FlintError::AudioError(format!("Sound not cached: {}", voice.name)))?
            .volume(voice.db)
            .playback_rate(kira::PlaybackRate(voice.rate))
            .panning(kira::Panning(voice.pan));
        if voice.looping {
            data = data.loop_region(..);
        }
        Ok(data)
    }
}

fn play_error(name: &str, e: impl std::fmt::Display) -> FlintError {
    FlintError::AudioError(format!("Failed to play '{name}': {e}"))
}

impl MixerBackend for Local {
    fn name(&self) -> &'static str {
        "main thread"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn is_remote(&self) -> bool {
        false
    }
    fn failed(&self) -> bool {
        false
    }
    fn needs_cache(&self) -> bool {
        true
    }
    // Plays from the engine's own copies.
    fn sound_cached(&mut self, _: &str, _: &StaticSoundData) {}
    fn holds_sound(&self, _: &str) -> bool {
        false
    }
    fn sound_names(&self) -> Vec<&str> {
        Vec::new()
    }
    fn sound_length(&self, _: &str) -> Option<f64> {
        None
    }

    fn add_bus(&mut self, bus: Bus, parent: Option<Bus>, db: Decibels) -> Result<()> {
        if bus.index() != self.buses.len() {
            return Err(FlintError::AudioError(format!(
                "bus {} made out of order",
                bus.index()
            )));
        }
        let builder = TrackBuilder::new().volume(db);
        let track = match parent {
            None => self.manager.add_sub_track(builder),
            Some(p) => {
                self.child_buses += 1;
                self.bus(p)?.add_sub_track(builder)
            }
        }
        .map_err(|e| FlintError::AudioError(format!("bus {}: {e}", bus.index())))?;
        self.buses.push(Some(track));
        Ok(())
    }

    fn set_bus_volume(&mut self, bus: Bus, db: Decibels, tween_secs: f64) {
        if let Ok(handle) = self.bus(bus) {
            handle.set_volume(db, tween_ms((tween_secs * 1000.0) as u64));
        }
    }

    fn set_filter_cutoff(&mut self, hz: f32, tween_secs: f64) {
        self.filter
            .set_cutoff(hz as f64, tween_ms((tween_secs * 1000.0) as u64));
    }

    fn create_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat) -> Result<()> {
        let handle = self
            .manager
            .add_listener(pos, orientation)
            .map_err(|e| FlintError::AudioError(format!("Failed to create listener: {e}")))?;
        self.listener = Some(handle);
        Ok(())
    }

    fn has_listener(&self) -> bool {
        self.listener.is_some()
    }

    fn move_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat) {
        if let Some(listener) = &mut self.listener {
            listener.set_position(pos, tween_ms(0));
            listener.set_orientation(orientation, tween_ms(0));
        }
    }

    fn create_spatial_track(
        &mut self,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<SpatialTrack> {
        let listener_id = self
            .listener
            .as_ref()
            .ok_or_else(|| FlintError::AudioError("No listener created".into()))?
            .id();
        let builder = SpatialTrackBuilder::new()
            .distances(SpatialTrackDistances {
                min_distance: falloff.min,
                max_distance: falloff.max,
            })
            .attenuation_function(Some(Easing::OutPowf(falloff.rolloff.max(0.01) as f64)));
        let handle = self
            .bus(bus)?
            .add_spatial_sub_track(listener_id, pos, builder)
            .map_err(|e| FlintError::AudioError(format!("Failed to create spatial track: {e}")))?;
        Ok(SpatialTrack::Local(Box::new(handle)))
    }

    fn play(&mut self, voice: Voice<'_>, on: On<'_>) -> Result<SoundHandle> {
        let data = Self::data(&voice)?;
        let handle = match on {
            On::Bus(bus) => self.bus(bus)?.play(data),
            On::Track(SpatialTrack::Local(track)) => track.play(data),
            On::Track(SpatialTrack::Remote(_)) => {
                return Err(FlintError::AudioError(
                    "the mix moved; this track is gone".into(),
                ))
            }
        }
        .map_err(|e| play_error(voice.name, e))?;
        Ok(SoundHandle::Local(handle))
    }

    fn play_on_own_track(
        &mut self,
        voice: Voice<'_>,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<Option<(SoundHandle, SpatialTrack)>> {
        let mut track = self.create_spatial_track(pos, falloff, bus)?;
        let handle = self.play(voice, On::Track(&mut track))?;
        Ok(Some((handle, track)))
    }

    // A local handle says when it has stopped.
    fn play_ends(&self, _: &str, _: f64, _: bool, _: Instant) -> Option<Instant> {
        None
    }

    // Each one-shot's own track is the engine's to drop.
    fn clear_oneshots(&mut self) {}

    fn add_counts(&self, counts: &mut MixCounts, held: &Held) -> Option<()> {
        for bus in self.buses.iter().flatten() {
            counts.voices += bus.num_sounds();
            counts.tracks += bus.num_sub_tracks();
        }
        counts.tracks = counts.tracks.saturating_sub(self.child_buses);
        counts.voices += held.on_tracks;
        Some(())
    }

    fn clock_now(&self, epoch: Instant) -> Option<f64> {
        Some(epoch.elapsed().as_secs_f64())
    }

    // kira's desktop stream doesn't say what its device adds.
    fn output_latency(&self) -> Option<f64> {
        None
    }

    fn host(&mut self) -> std::result::Result<&mut AudioManager<AudioBackend>, Unhosted> {
        Ok(&mut self.manager)
    }
}
