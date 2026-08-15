//! Milestone 0 suite playback: all non-silent buses started sample-locked on
//! one kira clock, one sub-track (with gain + low-pass) per bus.
//!
//! This is deliberately minimal — the seed of the Conductor, not the Conductor.
//! Known limitation, fine for Milestone 0: the clock runs at the *first* tempo
//! anchor's BPM; mid-suite tempo changes need the Phase 1 lookahead scheduler
//! driving `ClockHandle::set_speed` on anchor boundaries.

use crate::manifest::SuiteManifest;
use crate::TempoMap;
use flint_core::{FlintError, Result};
use kira::clock::{ClockHandle, ClockSpeed, ClockTime};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, StartTime};
use std::path::Path;

/// Beats of silence before sample 0, giving every stem's start a scheduled
/// future clock time (the same lookahead pattern reintegration will use).
pub const PREROLL_BEATS: u64 = 2;

pub struct StemBus {
    pub name: String,
    pub track: TrackHandle,
    pub filter: FilterHandle,
    pub sound: StaticSoundHandle,
}

pub struct SuitePlayer {
    // Field order matters: handles must drop before the manager.
    pub buses: Vec<StemBus>,
    pub clock: ClockHandle,
    pub tempo: TempoMap,
    _manager: AudioManager<DefaultBackend>,
}

impl SuitePlayer {
    /// Load every non-silent bus's stem, schedule all of them for the same
    /// clock tick, and start the clock. Validate the manifest *before* calling
    /// this; playback assumes a well-formed suite.
    pub fn start(manifest: &SuiteManifest, base_dir: &Path) -> Result<Self> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| FlintError::AudioError(format!("no audio device: {e}")))?;

        let bpm = manifest
            .tempo
            .first()
            .ok_or_else(|| FlintError::AudioError("manifest has no tempo anchors".into()))?
            .bpm;
        let mut clock = manager
            .add_clock(ClockSpeed::TicksPerMinute(bpm))
            .map_err(|e| FlintError::AudioError(format!("failed to create clock: {e}")))?;
        let start_at = StartTime::ClockTime(ClockTime::from_ticks_u64(&clock, PREROLL_BEATS));

        let mut buses = Vec::new();
        for (name, decl) in &manifest.buses {
            let Some(file) = &decl.file else { continue };
            let mut builder = TrackBuilder::new();
            let filter = builder.add_effect(
                FilterBuilder::new()
                    .mode(FilterMode::LowPass)
                    .cutoff(20_000.0),
            );
            let mut track = manager
                .add_sub_track(builder)
                .map_err(|e| FlintError::AudioError(format!("bus '{name}': {e}")))?;
            let data = StaticSoundData::from_file(base_dir.join(file))
                .map_err(|e| FlintError::AudioError(format!("bus '{name}' ({file}): {e}")))?
                .start_time(start_at);
            let sound = track
                .play(data)
                .map_err(|e| FlintError::AudioError(format!("bus '{name}': {e}")))?;
            buses.push(StemBus {
                name: name.clone(),
                track,
                filter,
                sound,
            });
        }
        if buses.is_empty() {
            return Err(FlintError::AudioError(
                "manifest declares no playable buses".into(),
            ));
        }

        clock.start();
        Ok(Self {
            buses,
            clock,
            tempo: TempoMap::new(manifest.tempo.clone(), manifest.sample_rate),
            _manager: manager,
        })
    }

    /// Beats since suite sample 0 (negative during pre-roll).
    pub fn beats(&self) -> f64 {
        let t = self.clock.time();
        t.ticks as f64 + t.fraction - PREROLL_BEATS as f64
    }

    /// Current suite sample position implied by the clock.
    pub fn sample_position(&self) -> i64 {
        let anchor = &self.tempo_anchor();
        let spb = self.tempo.sample_rate() as f64 * 60.0 / anchor.1;
        (self.beats() * spb) as i64
    }

    fn tempo_anchor(&self) -> (i64, f64) {
        // Milestone 0: single effective anchor (see module docs).
        self.tempo
            .anchor_at(0)
            .map(|a| (a.sample, a.bpm))
            .unwrap_or((0, 120.0))
    }

    /// Whether every stem has finished playing.
    pub fn all_stopped(&self) -> bool {
        self.buses
            .iter()
            .all(|b| b.sound.state() == kira::sound::PlaybackState::Stopped)
    }

    /// Max pairwise stem position disagreement, in seconds. Positions are
    /// polled back-to-back; expect ~0 while all stems are playing.
    pub fn max_stem_skew(&self) -> f64 {
        let positions: Vec<f64> = self.buses.iter().map(|b| b.sound.position()).collect();
        let mut max = 0.0f64;
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                max = max.max((positions[i] - positions[j]).abs());
            }
        }
        max
    }
}
