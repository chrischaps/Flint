//! Suite playback session: all buses started sample-locked on one clock,
//! backend-agnostic so the same code runs live (cpal) and offline (render
//! harness).
//!
//! Time authority: the kira clock counts **samples** (one tick per sample at
//! the manifest rate) and never changes speed. All musical interpretation —
//! tempo changes, meter changes, bars, beats — is the Conductor's arithmetic
//! over that sample count. One source of truth; the clock cannot drift from
//! the tempo map because it never speaks musical time at all.

use crate::conductor::Conductor;
use crate::manifest::{BusDecl, SuiteManifest};
use crate::mixer::BusMixer;
use crate::scheduler::{FiredEvent, Scheduler};
use flint_core::{FlintError, Result};
use kira::backend::Backend;
use kira::clock::{ClockHandle, ClockSpeed, ClockTime};
use kira::sound::static_sound::StaticSoundData;
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, StartTime};
use std::path::{Path, PathBuf};

/// Beats of silence before sample 0 (at the first anchor's tempo), giving
/// every stem's start a scheduled future clock time — the same lookahead
/// pattern reintegration entries will use.
pub const PREROLL_BEATS: f64 = 2.0;

/// Provides each bus's sound data. `FileStems` is the real implementation;
/// tests provide synthesized stems so tempo-change suites need no audio files.
pub trait StemResolver {
    /// `Ok(None)` for silent buses.
    fn load(&self, bus: &str, decl: &BusDecl) -> Result<Option<StaticSoundData>>;
}

/// Loads stems from disk relative to a base directory (the game repo root).
pub struct FileStems {
    pub base_dir: PathBuf,
}

impl FileStems {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
        }
    }
}

impl StemResolver for FileStems {
    fn load(&self, bus: &str, decl: &BusDecl) -> Result<Option<StaticSoundData>> {
        let Some(file) = &decl.file else {
            return Ok(None);
        };
        StaticSoundData::from_file(self.base_dir.join(file))
            .map(Some)
            .map_err(|e| FlintError::AudioError(format!("bus '{bus}' ({file}): {e}")))
    }
}

/// A running suite: mixer, clock, conductor, scheduler. Owns every kira
/// handle but not the `AudioManager` — the caller owns that (and its
/// backend), which is what lets the offline harness drive the same session.
/// Handles must drop before the manager; keep the manager alive longer.
pub struct SuiteSession {
    pub mixer: BusMixer,
    pub clock: ClockHandle,
    pub conductor: Conductor,
    pub scheduler: Scheduler,
    preroll_samples: u64,
}

impl SuiteSession {
    /// Load stems for all six buses, schedule them for one shared clock tick,
    /// and start the clock. Validate the manifest before calling; playback
    /// assumes a well-formed suite.
    pub fn start<B: Backend>(
        manifest: &SuiteManifest,
        manager: &mut AudioManager<B>,
        stems: &dyn StemResolver,
        latency_ms: Option<f64>,
    ) -> Result<Self> {
        let anchor = manifest
            .tempo
            .first()
            .ok_or_else(|| FlintError::AudioError("manifest has no tempo anchors".into()))?;
        let preroll_samples =
            (PREROLL_BEATS * manifest.sample_rate as f64 * 60.0 / anchor.bpm).round() as u64;

        let mut clock = manager
            .add_clock(ClockSpeed::TicksPerSecond(manifest.sample_rate as f64))
            .map_err(|e| FlintError::AudioError(format!("failed to create clock: {e}")))?;
        let start_at = StartTime::ClockTime(ClockTime::from_ticks_u64(&clock, preroll_samples));

        // Canonical BUSES order (not the BTreeMap's alphabetical order), with
        // any non-standard names the validator would have flagged appended.
        let mut names: Vec<&String> = manifest.buses.keys().collect();
        names.sort_by_key(|n| {
            crate::BUSES
                .iter()
                .position(|b| b == n)
                .unwrap_or(crate::BUSES.len())
        });
        let mut loaded = Vec::new();
        let mut playable = 0usize;
        for name in names {
            let data = stems.load(name, &manifest.buses[name])?;
            playable += data.is_some() as usize;
            loaded.push((name.clone(), data));
        }
        if playable == 0 {
            return Err(FlintError::AudioError(
                "manifest declares no playable buses".into(),
            ));
        }
        let mixer = BusMixer::build(manager, loaded, start_at)?;

        clock.start();
        Ok(Self {
            mixer,
            clock,
            conductor: Conductor::new(manifest, latency_ms),
            scheduler: Scheduler::new(Scheduler::default_horizon(manifest.sample_rate)),
            preroll_samples,
        })
    }

    /// Suite sample position implied by the clock (negative during pre-roll).
    /// The clock ticks once per sample, so this is exact, tempo-independent.
    pub fn clock_sample(&self) -> i64 {
        let t = self.clock.time();
        (t.ticks as f64 + t.fraction - self.preroll_samples as f64).floor() as i64
    }

    /// Latency-compensated musical position ("what the ear hears now").
    pub fn now(&self) -> crate::conductor::MusicalPosition {
        self.conductor.now(self.clock_sample())
    }

    /// Uncompensated musical position at the clock's play head.
    pub fn head(&self) -> crate::conductor::MusicalPosition {
        self.conductor.position_at_sample(self.clock_sample())
    }

    pub fn preroll_samples(&self) -> u64 {
        self.preroll_samples
    }

    /// Issue scheduled events entering the lookahead horizon. Call regularly
    /// (each poll tick live; each chunk offline).
    pub fn pump(&mut self) -> Vec<FiredEvent> {
        let now = self.clock_sample();
        self.scheduler
            .pump(now, &self.clock, self.preroll_samples, &mut self.mixer)
    }
}

/// Realtime playback on the default audio device. Field order matters:
/// the session's handles must drop before the manager.
pub struct RealtimePlayer {
    pub session: SuiteSession,
    _manager: AudioManager<DefaultBackend>,
}

impl RealtimePlayer {
    pub fn start(
        manifest: &SuiteManifest,
        base_dir: &Path,
        latency_ms: Option<f64>,
    ) -> Result<Self> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| FlintError::AudioError(format!("no audio device: {e}")))?;
        let session = SuiteSession::start(
            manifest,
            &mut manager,
            &FileStems::new(base_dir),
            latency_ms,
        )?;
        Ok(Self {
            session,
            _manager: manager,
        })
    }
}
