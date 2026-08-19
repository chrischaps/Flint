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
use crate::tempo::ms_to_samples;
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

    /// A degraded alternate's stem (ADR 0032). Default `Ok(None)`: resolvers
    /// that don't provide alternates leave them silently absent (the session
    /// warns), so test resolvers stay one-method.
    fn load_alternate(&self, _bus: &str, _file: &str) -> Result<Option<StaticSoundData>> {
        Ok(None)
    }
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

    fn load_alternate(&self, bus: &str, file: &str) -> Result<Option<StaticSoundData>> {
        StaticSoundData::from_file(self.base_dir.join(file))
            .map(Some)
            .map_err(|e| {
                FlintError::AudioError(format!("alternate for bus '{bus}' ({file}): {e}"))
            })
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
    /// Reintegration timeline map: `suite_sample = raw_clock_sample -
    /// timeline_offset`. The clock never rewinds; a seam that returns the
    /// suite to an earlier section adjusts this offset instead. 0 until the
    /// first seam.
    timeline_offset: i64,
    /// A seam whose audio is already scheduled but whose musical-time jump
    /// hasn't been reached yet: `(raw_sample_of_seam, new_offset)`. Committed
    /// by `pump` when the clock arrives.
    pending_jump: Option<(i64, i64)>,
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
        // Defensive: the validator rejects bpm <= 0 (coded issue), but a
        // caller can start an unvalidated manifest, and 0.0 here would
        // saturate the float→u64 cast to u64::MAX preroll — an unbounded
        // silent wait instead of an error.
        if !(anchor.bpm.is_finite() && anchor.bpm > 0.0) {
            return Err(FlintError::AudioError(format!(
                "first tempo anchor bpm {} is not positive/finite (validate the manifest)",
                anchor.bpm
            )));
        }
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
        let mut mixer = BusMixer::build(manager, loaded, start_at)?;

        // Composed degraded alternates (ADR 0032): full-length muted voices
        // started at the same shared tick — sample-locked by construction.
        for alt in &manifest.degraded_alternates {
            match stems.load_alternate(&alt.bus, &alt.file)? {
                Some(data) => {
                    mixer.attach_alternate(
                        &alt.bus,
                        data,
                        alt.from_sample,
                        alt.to_sample,
                        start_at,
                    )?;
                }
                None => tracing::warn!(
                    "degraded alternate for bus '{}' ({}) not provided by this resolver — \
                     rung crossfades to it will be silent no-ops",
                    alt.bus,
                    alt.file
                ),
            }
        }

        clock.start();
        Ok(Self {
            mixer,
            clock,
            conductor: Conductor::new(manifest, latency_ms),
            scheduler: Scheduler::new(Scheduler::default_horizon(manifest.sample_rate)),
            preroll_samples,
            timeline_offset: 0,
            pending_jump: None,
        })
    }

    /// Monotonic clock position in samples past pre-roll (negative during
    /// pre-roll). Never rewinds; seam bookkeeping lives on this timeline.
    pub fn raw_clock_sample(&self) -> i64 {
        let t = self.clock.time();
        (t.ticks as f64 + t.fraction - self.preroll_samples as f64).floor() as i64
    }

    /// Suite sample position implied by the clock: the raw clock position
    /// mapped through the reintegration timeline offset. This is the sample
    /// every musical consumer (conductor, judge, chart eval, replay) sees;
    /// it jumps backwards exactly once per seam.
    pub fn clock_sample(&self) -> i64 {
        self.raw_clock_sample() - self.timeline_offset
    }

    /// Map a raw clock sample to a suite sample under the current offset.
    pub fn suite_sample_for_raw(&self, raw: i64) -> i64 {
        raw - self.timeline_offset
    }

    pub fn timeline_offset(&self) -> i64 {
        self.timeline_offset
    }

    /// The kira clock time at a raw clock sample (raw = ticks − pre-roll).
    pub fn clock_time_at_raw(&self, raw: i64) -> kira::clock::ClockTime {
        kira::clock::ClockTime::from_ticks_u64(
            &self.clock,
            (raw + self.preroll_samples as i64).max(0) as u64,
        )
    }

    /// Schedule a reintegration seam: fade the current timeline to silence
    /// over `fade_ms` ending exactly at seam time `seam_suite_sample` (a
    /// sample on the *current* suite timeline, far enough ahead for the
    /// fade), re-play every stem from `re_entry_sample` at that same clock
    /// tick with per-bus entry levels, and queue the musical-time jump,
    /// which `pump` commits when the clock reaches the seam. The caller
    /// flushes the scheduler and owns the reassembly ramps; this method owns
    /// only sample-accurate mechanics.
    pub fn schedule_seam(
        &mut self,
        re_entry_sample: i64,
        seam_suite_sample: i64,
        fade_ms: f64,
        entry_db: &dyn Fn(&str) -> f32,
    ) -> Result<()> {
        let seam_raw = seam_suite_sample + self.timeline_offset;
        let seam_tick = seam_raw + self.preroll_samples as i64;
        if seam_tick < 0 || re_entry_sample < 0 {
            return Err(FlintError::AudioError(format!(
                "seam before timeline start (seam tick {seam_tick}, re-entry {re_entry_sample})"
            )));
        }
        let sample_rate = self.conductor.tempo().sample_rate() as f64;
        let fade_samples = ms_to_samples(fade_ms.max(0.0), sample_rate);
        let fade = kira::Tween {
            start_time: StartTime::ClockTime(self.clock_time_at_raw(seam_raw - fade_samples)),
            duration: std::time::Duration::from_secs_f64(fade_ms.max(0.0) / 1000.0),
            ..Default::default()
        };
        let start_at = StartTime::ClockTime(kira::clock::ClockTime::from_ticks_u64(
            &self.clock,
            seam_tick as u64,
        ));
        self.mixer.stop_all(fade);
        self.mixer.replay_all_at(re_entry_sample as u64, start_at, entry_db)?;
        self.pending_jump = Some((seam_raw, seam_raw - re_entry_sample));
        Ok(())
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

    /// Issue scheduled events entering the lookahead horizon, committing any
    /// pending seam jump the clock has reached. Call regularly (each poll
    /// tick live; each chunk offline).
    pub fn pump(&mut self) -> Vec<FiredEvent> {
        if let Some((seam_raw, offset)) = self.pending_jump {
            if self.raw_clock_sample() >= seam_raw {
                self.timeline_offset = offset;
                self.pending_jump = None;
            }
        }
        let now = self.clock_sample();
        let tick_base = self.preroll_samples as i64 + self.timeline_offset;
        self.scheduler
            .pump(now, &self.clock, tick_base, &mut self.mixer)
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
