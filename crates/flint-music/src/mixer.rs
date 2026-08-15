//! The six-bus stem mixer: runtime gain, low-pass, and detune per bus.
//!
//! Every bus in [`crate::BUSES`] gets a kira sub-track and filter — including
//! silent ones, which simply have no sound playing. Silent buses stay
//! addressable so later phases (reintegration entries, degraded alternates)
//! can bring material in without rebuilding the mixer.
//!
//! kira handles are write-only, so each bus keeps shadow state of the last
//! *commanded* values for introspection. During a tween the audible value lags
//! the shadow value by design; the readout reports intent, not the DSP state.

use flint_core::{FlintError, Result};
use kira::backend::Backend;
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, Decibels, PlaybackRate, StartTime, Tween};

/// Cutoff meaning "filter open" — matches the spike and Milestone 0.
pub const LPF_OPEN_HZ: f64 = 20_000.0;

/// Last commanded values for one bus (shadow state; see module docs).
#[derive(Debug, Clone, Copy)]
pub struct BusState {
    pub gain_db: f32,
    pub lpf_hz: f64,
    pub detune_semitones: f64,
    /// Whether this bus has a stem loaded (silent buses: false).
    pub playing: bool,
}

impl Default for BusState {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            lpf_hz: LPF_OPEN_HZ,
            detune_semitones: 0.0,
            playing: false,
        }
    }
}

pub struct StemBus {
    pub name: String,
    track: TrackHandle,
    filter: FilterHandle,
    sound: Option<StaticSoundHandle>,
    pub state: BusState,
}

impl StemBus {
    /// Set bus gain in dB. `tween.start_time` may be a future clock time —
    /// that is the scheduling hook every grid-aligned event uses.
    pub fn set_gain(&mut self, db: f32, tween: Tween) {
        self.track.set_volume(Decibels(db), tween);
        self.state.gain_db = db;
    }

    /// Set the low-pass cutoff in Hz ([`LPF_OPEN_HZ`] = effectively open).
    pub fn set_lpf(&mut self, hz: f64, tween: Tween) {
        self.filter.set_cutoff(hz, tween);
        self.state.lpf_hz = hz;
    }

    /// Detune in semitones via playback rate (per-sound — kira has no
    /// per-track pitch; see ADR 0001). Warble only: any nonzero detune breaks
    /// sample-lock against the other stems while active, so offline alignment
    /// asserts only hold at detune 0. No-op with a warning on silent buses.
    pub fn set_detune(&mut self, semitones: f64, tween: Tween) {
        match &mut self.sound {
            Some(sound) => {
                sound.set_playback_rate(PlaybackRate(2f64.powf(semitones / 12.0)), tween);
                self.state.detune_semitones = semitones;
            }
            None => tracing::warn!("set_detune on silent bus '{}' ignored", self.name),
        }
    }

    pub fn sound(&self) -> Option<&StaticSoundHandle> {
        self.sound.as_ref()
    }
}

/// All six buses, in canonical [`crate::BUSES`] order.
pub struct BusMixer {
    buses: Vec<StemBus>,
}

impl BusMixer {
    /// Build one sub-track + filter per bus and schedule each provided stem
    /// to start at `start_at` (one shared clock time = sample-locked start).
    /// `stems` pairs each bus name with its sound data, `None` for silent.
    pub fn build<B: Backend>(
        manager: &mut AudioManager<B>,
        stems: Vec<(String, Option<StaticSoundData>)>,
        start_at: StartTime,
    ) -> Result<Self> {
        let mut buses = Vec::new();
        for (name, data) in stems {
            let mut builder = TrackBuilder::new();
            let filter = builder.add_effect(
                FilterBuilder::new()
                    .mode(FilterMode::LowPass)
                    .cutoff(LPF_OPEN_HZ),
            );
            let mut track = manager
                .add_sub_track(builder)
                .map_err(|e| FlintError::AudioError(format!("bus '{name}': {e}")))?;
            let sound = match data {
                Some(data) => Some(
                    track
                        .play(data.start_time(start_at))
                        .map_err(|e| FlintError::AudioError(format!("bus '{name}': {e}")))?,
                ),
                None => None,
            };
            let state = BusState {
                playing: sound.is_some(),
                ..Default::default()
            };
            buses.push(StemBus {
                name,
                track,
                filter,
                sound,
                state,
            });
        }
        Ok(Self { buses })
    }

    pub fn bus_mut(&mut self, name: &str) -> Option<&mut StemBus> {
        self.buses.iter_mut().find(|b| b.name == name)
    }

    pub fn buses(&self) -> impl Iterator<Item = &StemBus> {
        self.buses.iter()
    }

    pub fn states(&self) -> impl Iterator<Item = (&str, &BusState)> {
        self.buses.iter().map(|b| (b.name.as_str(), &b.state))
    }

    /// Whether every loaded stem has finished playing.
    pub fn all_stopped(&self) -> bool {
        self.buses
            .iter()
            .filter_map(|b| b.sound.as_ref())
            .all(|s| s.state() == kira::sound::PlaybackState::Stopped)
    }

    /// Max pairwise stem position disagreement, in seconds. Positions are
    /// polled back-to-back; expect ~0 while all stems are playing.
    pub fn max_stem_skew(&self) -> f64 {
        let positions: Vec<f64> = self
            .buses
            .iter()
            .filter_map(|b| b.sound.as_ref())
            .map(|s| s.position())
            .collect();
        let mut max = 0.0f64;
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                max = max.max((positions[i] - positions[j]).abs());
            }
        }
        max
    }
}
