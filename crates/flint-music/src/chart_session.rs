//! One chart play-through, front-end agnostic: the reactive loop the Phase 2/3
//! harnesses run (chart-eval → judgment → coherence → ladder → reintegration
//! sequencer → mixer), hosted here so any binary — the CLI harness, the
//! windowed feel harness, the player app — can drive it (ADR 0016).
//!
//! Two layers:
//!
//! - [`ChartCore`] is the player-free loop body shared by the live session and
//!   the offline reactive render: it owns judgment, coherence, the ladder and
//!   reintegration sequencer, and the judgment log, and steps them against any
//!   [`SuiteSession`] — a realtime one or the offline renderer's.
//! - [`ChartSession`] is a live play-through: `ChartCore` plus a running
//!   [`SuiteSession`], the clock bridge, and the input-event receiver. The
//!   audio manager is either owned (the CLI path, [`ChartSession::open`]) or
//!   borrowed from a host for the duration of `start` (the player path,
//!   [`ChartSession::open_shared`] — ADR 0017's one-manager rule).
//!   Input arrives as a plain channel of [`InputEvent`]s; this crate never
//!   touches the gamepad backend (the capture thread lives upstream and its
//!   handle is parked here only as an opaque guard so teardown order holds).
//!
//! Front ends print; this module returns pre-formatted notice lines and
//! structured events instead of writing to stdout.

use crate::chart_eval::ChartEval;
use crate::clock_bridge::ClockBridge;
use crate::coherence::{Coherence, CoherenceConfig};
use crate::conductor::Conductor;
use crate::input_stream::InputEvent;
use crate::judgment::{Judge, JudgmentConfig, JudgmentRecord, JsonlWriter, LeanMode};
use crate::ladder::{Ladder, LadderConfig, LadderDriver, LadderParams};
use crate::reintegration::{ReintegrationEvent, Reintegrator, SeqTick};
use crate::replay::{SessionHeader, SessionWriter};
use crate::session::{FileStems, SuiteSession};
use crate::status::status_line_with_coherence;
use crate::{validate_chart, validate_manifest, validate_manifest_assets, Chart, SuiteManifest};
use flint_core::{FlintError, Result};
use kira::backend::Backend;
use kira::{AudioManager, AudioManagerSettings, DefaultBackend};
use std::path::{Path, PathBuf};
use std::time::Instant;

// --- timing-offset files ----------------------------------------------------
// Reading the committed logs in `<base_dir>/logs/latency/`: output-latency
// measurements (`latency-*.toml`, from spikes/latency_harness) and tap-to-beat
// calibrations (`calibration-*.toml`, from `flint calibrate`). File names
// embed date/epoch so lexicographic max = newest. Both offsets are echoed
// into every judgment/session header so runs are reproducible.

/// Newest output-latency measurement: `(file_name, ms)`. Prefers the
/// loopback mean, falling back to the driver-reported mean.
pub fn latest_latency_ms(base_dir: &Path) -> Option<(String, f64)> {
    let (name, value) = newest_toml(base_dir, "latency-")?;
    let mean = |table: &str| {
        value
            .get(table)
            .and_then(|t| t.get("mean"))
            .and_then(flint_core::toml_util::toml_f64)
            .filter(|m| m.is_finite())
    };
    let ms = mean("loopback_ms").or_else(|| mean("driver_reported_ms"))?;
    Some((name, ms))
}

/// Newest tap-to-beat calibration: `(file_name, median_ms)`. Positive =
/// the player's taps land late relative to the latency-compensated grid.
pub fn latest_calibration_ms(base_dir: &Path) -> Option<(String, f64)> {
    let (name, value) = newest_toml(base_dir, "calibration-")?;
    let ms = value
        .get("calibration")
        .and_then(|t| t.get("median_ms"))
        .and_then(flint_core::toml_util::toml_f64)
        .filter(|m| m.is_finite())?;
    Some((name, ms))
}

fn newest_toml(base_dir: &Path, prefix: &str) -> Option<(String, toml::Value)> {
    let dir = base_dir.join("logs/latency");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".toml"))
        .collect();
    names.sort();
    let name = names.pop()?;
    let value: toml::Value = std::fs::read_to_string(dir.join(&name)).ok()?.parse().ok()?;
    Some((name, value))
}

/// Total judgment offset (measured output latency + tap calibration) in
/// samples — the number the input capture subtracts at the stamp. One
/// definition so the CLI harness and the player app can never disagree.
pub fn judgment_offset_samples(
    latency_ms: Option<f64>,
    calibration_ms: f64,
    sample_rate: u32,
) -> i64 {
    ((latency_ms.unwrap_or(0.0) + calibration_ms) / 1000.0 * sample_rate as f64).round() as i64
}

// --- lean mode --------------------------------------------------------------

/// Shared by play-chart and replay-chart so live and offline judge alike.
pub fn parse_lean_mode(s: &str) -> Result<LeanMode> {
    match s {
        "arrival" => Ok(LeanMode::Arrival),
        "track" => Ok(LeanMode::Track),
        other => Err(FlintError::ValidationError(format!(
            "unknown --lean-mode `{other}` (expected `arrival` or `track`)"
        ))),
    }
}

pub fn lean_mode_name(mode: LeanMode) -> &'static str {
    match mode {
        LeanMode::Arrival => "arrival",
        LeanMode::Track => "track",
    }
}

// --- coherence-config resolution --------------------------------------------

/// Where a resolved coherence config came from — the front end formats its
/// own notice from this (live and replay word theirs differently).
pub enum CoherenceSource {
    Explicit(PathBuf),
    /// A recorded session header's config snapshot.
    Snapshot,
    DefaultFile(PathBuf),
    /// Built-in defaults; carries the default path that was absent.
    Builtin(PathBuf),
}

/// Resolution chain: explicit path (must load) > session-header snapshot >
/// `<base_dir>/config/coherence.toml` when present > built-in defaults.
pub fn resolve_coherence_config(
    explicit: Option<&Path>,
    snapshot: Option<&serde_json::Value>,
    base_dir: &Path,
) -> Result<(CoherenceConfig, CoherenceSource)> {
    if let Some(path) = explicit {
        let cfg = CoherenceConfig::load(path)
            .map_err(|e| FlintError::ValidationError(format!("loading {}: {e}", path.display())))?;
        Ok((cfg, CoherenceSource::Explicit(path.to_path_buf())))
    } else if let Some(snap) = snapshot {
        Ok((CoherenceConfig::from_json(snap), CoherenceSource::Snapshot))
    } else {
        let default_path = base_dir.join("config/coherence.toml");
        if default_path.exists() {
            let cfg = CoherenceConfig::load(&default_path)?;
            Ok((cfg, CoherenceSource::DefaultFile(default_path)))
        } else {
            Ok((CoherenceConfig::default(), CoherenceSource::Builtin(default_path)))
        }
    }
}

// --- the player-free core ---------------------------------------------------

/// End-of-run tallies, formatted by the front end.
pub struct FinishSummary {
    pub hits: u64,
    pub misses: u64,
    pub spurious: u64,
    pub mean_abs_err_ms: f64,
}

/// The loop body shared by the live harness, the offline reactive render, and
/// (eventually) the player app: everything one play-through owns *except* the
/// audio backend and the input source. Callers feed it events and a
/// [`SuiteSession`]; it judges, integrates coherence, drives the ladder and
/// reintegration sequencer, and writes the judgment log.
pub struct ChartCore {
    judge: Judge,
    coherence: Coherence,
    /// Musical time per lean record, precomputed for coherence's integrator.
    lean_step_beats: f64,
    log: JsonlWriter,
    ladder: Ladder,
    ladder_driver: LadderDriver,
    reintegrator: Reintegrator,
    /// Tempo authority for beats-per-bar lookups (latency plays no role).
    conductor: Conductor,
    config_path: PathBuf,
    ladder_path: PathBuf,
    records: Vec<JudgmentRecord>,
    seq_events: Vec<ReintegrationEvent>,
    hits: u64,
    misses: u64,
    spurious: u64,
    abs_err_sum_ms: f64,
}

impl ChartCore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        judge: Judge,
        coherence: Coherence,
        log: JsonlWriter,
        ladder: Ladder,
        reintegrator: Reintegrator,
        conductor: Conductor,
        config_path: PathBuf,
        ladder_path: PathBuf,
    ) -> Self {
        let lean_step_beats = judge.lean_step_beats();
        Self {
            judge,
            coherence,
            lean_step_beats,
            log,
            ladder,
            ladder_driver: LadderDriver::new(),
            reintegrator,
            conductor,
            config_path,
            ladder_path,
            records: Vec::new(),
            seq_events: Vec::new(),
            hits: 0,
            misses: 0,
            spurious: 0,
            abs_err_sum_ms: 0.0,
        }
    }

    /// Wire the seam parameters from the ladder config into the sequencer.
    /// Called on construction and after a ladder reload.
    pub fn sync_seam_params(&mut self) {
        self.reintegrator.fade_ms = self.ladder.config().seam_fade_ms;
        self.reintegrator.rewind_beats = self.ladder.config().seam_rewind_beats;
        self.reintegrator.rewind_drop_st = self.ladder.config().seam_rewind_drop_st;
        self.reintegrator.pickup_beats = self.ladder.config().seam_pickup_beats;
    }

    /// Feed one suite-stamped event to judgment (non-positive-time events are
    /// the caller's to filter; this ingests unconditionally).
    pub fn ingest(&mut self, ev: &InputEvent) {
        self.judge.ingest(ev, &mut self.records);
    }

    /// Expire due windows up to `now_suite`.
    pub fn advance_to(&mut self, now_suite: i64) {
        self.judge.advance_to(now_suite, &mut self.records);
    }

    /// Step coherence over the pending records, tally, and write them to the
    /// judgment log. Returns the pulse errors judged this step (the live
    /// front end echoes each; offline stays quiet).
    pub fn process(&mut self, at_sample: i64) -> Result<Vec<f64>> {
        let mut pulse_errs = Vec::new();
        if self.records.is_empty() {
            return Ok(pulse_errs);
        }
        let beats_per_bar = self.beats_per_bar(at_sample.max(0));
        let value = self
            .coherence
            .step(&self.records, self.lean_step_beats, beats_per_bar);
        self.log.write_value(&serde_json::json!({
            "t": "coherence", "sample": at_sample, "value": value,
        }))?;
        for rec in self.records.drain(..) {
            match &rec {
                JudgmentRecord::Pulse { err_ms, .. } => {
                    self.hits += 1;
                    self.abs_err_sum_ms += err_ms.abs();
                    pulse_errs.push(*err_ms);
                }
                JudgmentRecord::Miss { .. } => self.misses += 1,
                JudgmentRecord::Spurious { .. } => self.spurious += 1,
                JudgmentRecord::Track { .. } => {}
            }
            self.log.write(&rec)?;
        }
        Ok(pulse_errs)
    }

    /// One tick of the disintegration ladder + reintegration sequencer:
    /// coherence in, mixer tweens + visual params out. Handles the judge
    /// rewind and log records for every sequencer event, then hands the
    /// events back for front-end formatting (and session-file jump records).
    /// Call only past pre-roll, after `session.pump()`.
    pub fn step_seq(
        &mut self,
        session: &mut SuiteSession,
    ) -> Result<(SeqTick, Vec<ReintegrationEvent>)> {
        let seq = self.reintegrator.tick(
            self.coherence.value(),
            &mut self.ladder,
            &mut self.ladder_driver,
            session,
            &mut self.seq_events,
        )?;
        let events = std::mem::take(&mut self.seq_events);
        for ev in &events {
            match ev {
                ReintegrationEvent::FullFail {
                    at_suite_sample,
                    re_entry_sample,
                    seam_suite_sample,
                } => {
                    self.log.write_value(&serde_json::json!({
                        "t": "full_fail", "sample": at_suite_sample,
                        "re_entry_sample": re_entry_sample,
                        "seam_suite_sample": seam_suite_sample,
                    }))?;
                }
                ReintegrationEvent::Seam {
                    raw_sample,
                    timeline_offset,
                    re_entry_sample,
                } => {
                    self.judge.rewind_to(*re_entry_sample);
                    self.log.write_value(&serde_json::json!({
                        "t": "seam", "raw_sample": raw_sample,
                        "timeline_offset": timeline_offset,
                        "re_entry_sample": re_entry_sample,
                    }))?;
                }
                ReintegrationEvent::ReassemblyComplete { at_suite_sample } => {
                    self.log.write_value(&serde_json::json!({
                        "t": "reassembly_complete", "sample": at_suite_sample,
                    }))?;
                }
            }
        }
        Ok((seq, events))
    }

    /// Reload the coherence config (and the ladder config when its file
    /// exists) from the paths resolved at open. Value/level carry over.
    /// `sample` stamps the reload records. Returns notice lines.
    pub fn reload_config(&mut self, sample: i64) -> Result<Vec<String>> {
        let mut notices = Vec::new();
        match CoherenceConfig::load(&self.config_path) {
            Ok(cfg) => {
                self.coherence.reconfigure(cfg);
                notices.push("coherence config reloaded (value carries over)".to_string());
                self.log.write_value(&serde_json::json!({
                    "t": "config_reload", "sample": sample,
                    "coherence_config": cfg.to_json(),
                }))?;
            }
            Err(e) => notices.push(format!("reload failed, keeping current config: {e}")),
        }
        if self.ladder_path.exists() {
            match LadderConfig::load(&self.ladder_path) {
                Ok(cfg) => {
                    self.ladder.reconfigure(cfg);
                    self.sync_seam_params();
                    notices.push("ladder config reloaded (level carries over)".to_string());
                    self.log.write_value(&serde_json::json!({
                        "t": "ladder_reload", "sample": sample,
                        "ladder_config": self.ladder.config().to_json(),
                    }))?;
                }
                Err(e) => notices.push(format!("ladder reload failed, keeping current config: {e}")),
            }
        }
        Ok(notices)
    }

    /// Expire every outstanding window into the pending records WITHOUT
    /// tallying them — for callers that want the final records to go through
    /// [`ChartCore::process`] (and so through coherence), the way the plain
    /// replay always has. Follow with `process(final_sample)` + `flush_log`.
    pub fn judge_finish(&mut self) {
        self.judge.finish(&mut self.records);
    }

    /// The tallies so far (the plain-replay path reads them after its final
    /// `process`; the live path gets them from [`ChartCore::finish`]).
    pub fn summary(&self) -> FinishSummary {
        let mean_abs_err_ms = if self.hits > 0 {
            self.abs_err_sum_ms / self.hits as f64
        } else {
            0.0
        };
        FinishSummary {
            hits: self.hits,
            misses: self.misses,
            spurious: self.spurious,
            mean_abs_err_ms,
        }
    }

    /// Close judgment: expire everything outstanding, flush the log, tally.
    /// (Final records are counted but do not step coherence — live-harness
    /// semantics.)
    pub fn finish(&mut self) -> Result<FinishSummary> {
        self.judge.finish(&mut self.records);
        for rec in self.records.drain(..) {
            match &rec {
                JudgmentRecord::Miss { .. } => self.misses += 1,
                JudgmentRecord::Spurious { .. } => self.spurious += 1,
                _ => {}
            }
            self.log.write(&rec)?;
        }
        self.log.flush()?;
        let mean_abs_err_ms = if self.hits > 0 {
            self.abs_err_sum_ms / self.hits as f64
        } else {
            0.0
        };
        Ok(FinishSummary {
            hits: self.hits,
            misses: self.misses,
            spurious: self.spurious,
            mean_abs_err_ms,
        })
    }

    pub fn flush_log(&mut self) -> Result<()> {
        self.log.flush()
    }

    pub fn coherence(&self) -> &Coherence {
        &self.coherence
    }

    pub fn ladder(&self) -> &Ladder {
        &self.ladder
    }

    fn beats_per_bar(&self, sample: i64) -> f64 {
        self.conductor
            .tempo()
            .anchor_at(sample)
            .map(|a| a.beats_per_bar as f64)
            .unwrap_or(4.0)
    }
}

// --- the live session -------------------------------------------------------

/// How to open a [`ChartSession`]. Paths are caller-resolved; the latency and
/// calibration offsets come from the caller too (see [`latest_latency_ms`] /
/// [`latest_calibration_ms`]) so the caller can also configure its input
/// source with the same numbers.
pub struct ChartSessionConfig {
    pub manifest: PathBuf,
    pub chart: PathBuf,
    pub base_dir: PathBuf,
    /// Explicit coherence-config path (must load); default is
    /// `<base_dir>/config/coherence.toml` when present, else built-ins.
    pub coherence_config: Option<PathBuf>,
    /// Explicit ladder-config path (same contract as the coherence config).
    pub ladder_config: Option<PathBuf>,
    /// Record the input session to `logs/sessions/<name>.session.jsonl`.
    pub record: Option<String>,
    /// Stop after this many bars (default: play the suite out).
    pub bars: Option<u64>,
    pub lean_mode: LeanMode,
    /// Measured output latency (ms), if any is on record.
    pub latency_ms: Option<f64>,
    /// Tap-to-beat calibration offset (ms); 0 when none is on record.
    pub calibration_ms: f64,
}

#[derive(PartialEq)]
pub enum Tick {
    Running,
    Finished,
}

/// One [`ChartSession::tick`]'s outcome: the run state plus notice lines for
/// the front end to print verbatim (pulse echoes, sequencer events, the
/// per-bar status line). No notice ever carries gameplay judgment for the
/// player's eyes — these are dev-harness surfaces.
pub struct TickOutcome {
    pub state: Tick,
    pub notices: Vec<String>,
}

/// Snapshot the window shader draws from. No numbers reach the screen —
/// these drive glows and breathing, not meters.
pub struct VisualFrame {
    /// 0..1 within the current beat / bar (0 at the boundary).
    pub beat_phase: f32,
    pub bar_phase: f32,
    /// Player lean (deadzoned) and the chart's lean target, both in [-1,1]².
    pub lean: [f32; 2],
    pub target: [f32; 2],
    pub coherence: f32,
    /// Seconds since the player's most recent pulse press (large if none).
    pub pulse_age_s: f32,
    pub preroll: bool,
    /// Ladder visual params (0 = clean). Mirrors the audio rungs exactly —
    /// one param source, so the world comes apart as one thing.
    pub desaturate: f32,
    pub blur: f32,
    pub chromatic: f32,
    /// Reassembly progress: 1 = normal play; during reintegration it runs
    /// 0 → 1 as the world re-gathers (the visual chain in reverse).
    pub reassembly: f32,
    /// Debug cue: capture is running but has never delivered an event —
    /// the ADR 0011 silent failure, surfaced in the harness window.
    pub no_input: bool,
    /// Rewind interlude progress: 0 = not rewinding; 0→1 while the record
    /// spins backwards toward the seam.
    pub rewind: f32,
}

/// Everything one live play-through owns, front-end agnostic. A front end
/// calls [`ChartSession::tick`] at its own cadence (the audio clock is the
/// master clock — ticking merely drains queues and advances judgment), plus
/// [`ChartSession::reload_config`] / [`ChartSession::finish`] on its own
/// input gestures.
pub struct ChartSession {
    core: ChartCore,
    session_writer: Option<(SessionWriter, PathBuf)>,
    bridge: ClockBridge,
    /// Opaque guard for the input producer (the capture thread's handle).
    /// Declared before `session` so the producer stops before the audio
    /// handles on any drop path, not just [`ChartSession::finish`].
    input_guard: Option<Box<dyn std::any::Any + Send>>,
    input_rx: Option<std::sync::mpsc::Receiver<InputEvent>>,
    /// The running suite (kira handles only). Declared after the input guard
    /// and before `own_manager` so declaration-order drop runs producer →
    /// handles → device (ADR 0017).
    session: SuiteSession,
    /// Params currently applied — the visual half feeds `visual_frame`.
    ladder_params: LadderParams,
    /// Reassembly progress for visuals (1.0 in normal play).
    reassembly: f64,
    /// Rewind interlude progress for visuals (0 outside the interlude).
    rewind: f64,
    end_sample: i64,
    sample_rate: u32,
    /// Second evaluator instance, kept out of the judge for visual lookups.
    visual_eval: ChartEval,
    last_bar: i64,
    /// Total input events received; a live capture that delivers nothing is
    /// the failure mode worth shouting about (ADR 0011).
    input_events: u64,
    warned_no_input: bool,
    lean: [f64; 2],
    last_pulse_at: Option<Instant>,
    /// The audio manager when this session owns one (the CLI path). `None`
    /// on the shared-manager path, where the host's manager outlives us.
    /// Last field: everything above (session included) drops before it.
    _own_manager: Option<AudioManager<DefaultBackend>>,
}

impl ChartSession {
    /// Validate, start audio on a manager of our own, and assemble the whole
    /// reactive loop — the CLI path. Returns the session plus startup notice
    /// lines. The session has no input until the caller attaches a receiver
    /// via [`ChartSession::set_input`].
    pub fn open(cfg: &ChartSessionConfig) -> Result<(Self, Vec<String>)> {
        let (manifest, chart) = Self::validate(cfg)?;
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| FlintError::AudioError(format!("no audio device: {e}")))?;
        let session = SuiteSession::start(
            &manifest,
            &mut manager,
            &FileStems::new(&cfg.base_dir),
            cfg.latency_ms,
        )?;
        Self::open_with(cfg, &manifest, &chart, session, Some(manager))
    }

    /// Same as [`ChartSession::open`] but the suite starts on a caller-owned
    /// manager (the player app's device stream, ADR 0017). The borrow ends
    /// at start — the session keeps only kira handles, and the caller must
    /// keep the manager alive longer than the returned session.
    pub fn open_shared<B: Backend>(
        cfg: &ChartSessionConfig,
        manager: &mut AudioManager<B>,
    ) -> Result<(Self, Vec<String>)> {
        let (manifest, chart) = Self::validate(cfg)?;
        let session = SuiteSession::start(
            &manifest,
            manager,
            &FileStems::new(&cfg.base_dir),
            cfg.latency_ms,
        )?;
        Self::open_with(cfg, &manifest, &chart, session, None)
    }

    /// Load and validate the manifest + chart before any audio is touched,
    /// so the failure path is identical on both open fronts.
    fn validate(cfg: &ChartSessionConfig) -> Result<(SuiteManifest, Chart)> {
        let manifest = SuiteManifest::load(&cfg.manifest)
            .map_err(|e| FlintError::ValidationError(format!("loading {}: {e}", cfg.manifest.display())))?;
        let chart = Chart::load(&cfg.chart)
            .map_err(|e| FlintError::ValidationError(format!("loading {}: {e}", cfg.chart.display())))?;
        let mut issues = validate_manifest(&manifest);
        issues.extend(validate_manifest_assets(&manifest, &cfg.base_dir));
        issues.extend(validate_chart(&chart, &manifest));
        if !issues.is_empty() {
            let mut msg = String::new();
            for i in &issues {
                msg.push_str(&format!("{i}\n"));
            }
            msg.push_str(&format!(
                "validation failed ({} issue(s)); not playing",
                issues.len()
            ));
            return Err(FlintError::ValidationError(msg));
        }
        Ok((manifest, chart))
    }

    /// Everything downstream of audio start: judgment, coherence, ladder,
    /// logs, bookkeeping. Shared by both open fronts.
    fn open_with(
        cfg: &ChartSessionConfig,
        manifest: &SuiteManifest,
        chart: &Chart,
        session: SuiteSession,
        own_manager: Option<AudioManager<DefaultBackend>>,
    ) -> Result<(Self, Vec<String>)> {
        let mut notices = Vec::new();
        // Judgment arithmetic is pure: event samples arrive already compensated
        // (capture subtracts the offset), so its conductor carries none.
        let judge_conductor = Conductor::new(manifest, None);
        let visual_eval = ChartEval::new(chart, &judge_conductor)?;
        let eval = ChartEval::new(chart, &judge_conductor)?;
        let judgment_cfg = JudgmentConfig {
            lean_mode: cfg.lean_mode,
            ..Default::default()
        };
        notices.push(format!("lean mode: {}", lean_mode_name(judgment_cfg.lean_mode)));
        let judge = Judge::new(eval, judge_conductor, judgment_cfg);

        // Coherence config: explicit path must load; the default path is
        // optional. `r`+Enter (or `r` in the window) reloads it mid-session.
        let (coherence_cfg, source) =
            resolve_coherence_config(cfg.coherence_config.as_deref(), None, &cfg.base_dir)?;
        let config_path = match &source {
            CoherenceSource::Explicit(p)
            | CoherenceSource::DefaultFile(p)
            | CoherenceSource::Builtin(p) => p.clone(),
            CoherenceSource::Snapshot => unreachable!("no snapshot source in a live session"),
        };
        match &source {
            CoherenceSource::Explicit(p) | CoherenceSource::DefaultFile(p) => notices.push(format!(
                "coherence config: {} (r+Enter = reload, q+Enter = quit)",
                p.display()
            )),
            CoherenceSource::Builtin(p) => notices.push(format!(
                "coherence config: built-in defaults (no {})",
                p.display()
            )),
            CoherenceSource::Snapshot => {}
        }
        let coherence = Coherence::new(coherence_cfg);

        // Ladder config: same contract as the coherence config — explicit
        // path must load, default path is optional, reloads with `r`.
        let ladder_explicit = cfg.ladder_config.is_some();
        let ladder_path = cfg
            .ladder_config
            .clone()
            .unwrap_or_else(|| cfg.base_dir.join("config/ladder.toml"));
        let ladder_cfg = if ladder_explicit || ladder_path.exists() {
            let lcfg = LadderConfig::load(&ladder_path).map_err(|e| {
                FlintError::ValidationError(format!("loading {}: {e}", ladder_path.display()))
            })?;
            notices.push(format!(
                "ladder config: {} ({} rung(s))",
                ladder_path.display(),
                lcfg.rungs.len()
            ));
            lcfg
        } else {
            notices.push(format!(
                "ladder config: built-in defaults (no {})",
                ladder_path.display()
            ));
            LadderConfig::default()
        };
        let ladder = Ladder::new(ladder_cfg);
        let reintegrator = Reintegrator::new(manifest.reintegration.clone());

        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log_path = cfg.base_dir.join(format!("logs/judgment/judgment-{epoch}.jsonl"));
        let header = serde_json::json!({
            "t": "header", "schema": 0,
            "suite": manifest.id, "chart": cfg.chart.display().to_string(),
            "latency_ms": cfg.latency_ms.unwrap_or(0.0), "calibration_ms": cfg.calibration_ms,
            "grid_beats": judgment_cfg.grid_beats,
            "lean_mode": lean_mode_name(judgment_cfg.lean_mode),
            "arrival_half_beats": judgment_cfg.arrival_half_beats,
            "coherence_config": coherence_cfg.to_json(),
            "ladder_config": ladder.config().to_json(),
            "epoch_s": epoch,
        });
        let log = JsonlWriter::create(&log_path, &header)?;
        notices.push(format!("judgment log: {}", log_path.display()));

        let session_writer = match &cfg.record {
            Some(name) => {
                let path = cfg
                    .base_dir
                    .join(format!("logs/sessions/{name}.session.jsonl"));
                let session_header = SessionHeader {
                    schema: 0,
                    suite: manifest.id.clone(),
                    chart: cfg.chart.display().to_string(),
                    sample_rate: manifest.sample_rate,
                    latency_ms: cfg.latency_ms.unwrap_or(0.0),
                    calibration_ms: cfg.calibration_ms,
                    coherence_config: Some(coherence_cfg.to_json()),
                    capture: Some(serde_json::json!({
                        "poll_hz": 1000, "deadzone": 0.12,
                    })),
                };
                let w = SessionWriter::create(&path, &session_header)?;
                notices.push(format!("recording session: {}", path.display()));
                Some((w, path))
            }
            None => None,
        };

        let bridge = ClockBridge::new(manifest.sample_rate);

        let end_sample = cfg
            .bars
            .map(|b| session.conductor.sample_at_bar(b as i64, 0.0))
            .unwrap_or(i64::MAX);
        let sample_rate = manifest.sample_rate;

        let mut core = ChartCore::new(
            judge,
            coherence,
            log,
            ladder,
            reintegrator,
            Conductor::new(manifest, None),
            config_path,
            ladder_path,
        );
        core.sync_seam_params();

        Ok((
            Self {
                core,
                session,
                session_writer,
                bridge,
                input_guard: None,
                input_rx: None,
                ladder_params: LadderParams::clean(),
                reassembly: 1.0,
                rewind: 0.0,
                end_sample,
                sample_rate,
                visual_eval,
                last_bar: i64::MIN,
                input_events: 0,
                warned_no_input: false,
                lean: [0.0; 2],
                last_pulse_at: None,
                _own_manager: own_manager,
            },
            notices,
        ))
    }

    /// A clone of the clock bridge for the input producer to stamp events
    /// with (fed raw clock samples by every `tick`).
    pub fn bridge(&self) -> ClockBridge {
        self.bridge.clone()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Attach the input source: the event receiver plus an opaque guard for
    /// the producer (dropped in `finish` before audio teardown).
    pub fn set_input(
        &mut self,
        rx: std::sync::mpsc::Receiver<InputEvent>,
        guard: Box<dyn std::any::Any + Send>,
    ) {
        self.input_rx = Some(rx);
        self.input_guard = Some(guard);
    }

    /// One iteration: pump the scheduler, feed the clock bridge, drain input
    /// into judgment, step coherence, log, and emit the per-bar status line.
    pub fn tick(&mut self) -> Result<TickOutcome> {
        self.tick_with(|_| {})
    }

    /// [`ChartSession::tick`] with a per-event tap: `on_event` sees every
    /// drained input event, raw-stamped, before the timeline-offset remap.
    /// The player app down-samples this into its frame-quantized `InputState`
    /// (ADR 0018); the judge consumes the same events at full precision.
    pub fn tick_with(&mut self, mut on_event: impl FnMut(&InputEvent)) -> Result<TickOutcome> {
        let mut notices = Vec::new();
        self.session.pump();
        // The bridge fits a line through clock observations — it must see the
        // monotonic RAW clock; a seam's backwards jump would poison the fit.
        // Events therefore arrive stamped in raw samples and are re-mapped to
        // suite time at ingest.
        self.bridge
            .observe(Instant::now(), self.session.raw_clock_sample());

        let pos = self.session.now();

        let timeline_offset = self.session.timeline_offset();
        if let Some(rx) = &self.input_rx {
            while let Ok(ev) = rx.try_recv() {
                self.input_events += 1;
                on_event(&ev);
                match &ev {
                    InputEvent::Lean(l) => {
                        self.lean = [l.x, l.y];
                    }
                    InputEvent::Pulse(_) => {
                        self.last_pulse_at = Some(Instant::now());
                    }
                }
                // Session files record raw samples (monotonic across seams;
                // jump records reconstruct suite time — ADR 0009/0014).
                if ev.sample() >= 0 {
                    if let Some((w, _)) = &mut self.session_writer {
                        w.write(&ev, self.sample_rate)?;
                    }
                }
                let suite_ev = if timeline_offset != 0 {
                    ev.with_sample(ev.sample() - timeline_offset)
                } else {
                    ev
                };
                if suite_ev.sample() >= 0 {
                    self.core.ingest(&suite_ev);
                }
            }
        }
        if pos.sample >= 0 {
            self.core.advance_to(pos.sample);
        }
        for err_ms in self.core.process(pos.sample)? {
            notices.push(format!("  pulse {err_ms:+.1} ms"));
        }

        if pos.sample < 0 {
            return Ok(TickOutcome {
                state: Tick::Running,
                notices,
            }); // pre-roll
        }

        // Disintegration ladder + reintegration sequencer: coherence in,
        // mixer tweens + visual params out. Idle most ticks (hysteresis).
        let (seq, seq_events) = self.core.step_seq(&mut self.session)?;
        self.ladder_params = seq.params;
        self.reassembly = seq.reassembly;
        self.rewind = seq.rewind;
        for ev in seq_events {
            match &ev {
                ReintegrationEvent::FullFail {
                    at_suite_sample,
                    re_entry_sample,
                    seam_suite_sample,
                } => {
                    notices.push(format!(
                        "  FULL FAIL at sample {at_suite_sample}: reintegrating to sample \
                         {re_entry_sample} (seam at {seam_suite_sample})"
                    ));
                }
                ReintegrationEvent::Seam {
                    raw_sample,
                    timeline_offset,
                    re_entry_sample,
                } => {
                    notices.push(format!(
                        "  seam: world re-gathering from sample {re_entry_sample}"
                    ));
                    if let Some((w, _)) = &mut self.session_writer {
                        w.write_timeline_jump(*raw_sample, *timeline_offset)?;
                    }
                }
                ReintegrationEvent::ReassemblyComplete { .. } => {
                    notices.push("  reassembly complete".to_string());
                }
            }
        }

        if pos.sample >= self.end_sample || self.session.mixer.all_stopped() {
            return Ok(TickOutcome {
                state: Tick::Finished,
                notices,
            });
        }

        if pos.bar != self.last_bar {
            self.last_bar = pos.bar;
            // A capture thread that delivers nothing is silent by nature —
            // shout once, early, while the session is still salvageable.
            if !self.warned_no_input
                && self.input_rx.is_some()
                && self.input_events == 0
                && pos.bar >= 2
            {
                self.warned_no_input = true;
                notices.push(format!(
                    "  *** NO INPUT RECEIVED after {} bars — capture is running but the \
                     controller is delivering nothing.\n  *** Check the controller/mapper \
                     mode (XInput), then restart the session.",
                    pos.bar
                ));
            }
            let section = self.session.conductor.section_at_sample(pos.sample);
            let no_input = if self.input_rx.is_some() && self.input_events == 0 {
                " | NO INPUT"
            } else {
                ""
            };
            let rung = match self.core.ladder().rung() {
                Some(r) => format!(" | rung {} ({})", self.core.ladder().level(), r.name),
                None => String::new(),
            };
            notices.push(format!(
                "{} | lean ({:+.2},{:+.2}){rung}{no_input}",
                status_line_with_coherence(
                    &pos,
                    section,
                    &self.session.mixer,
                    self.core.coherence().value()
                ),
                self.lean[0],
                self.lean[1],
            ));
            self.core.flush_log()?;
        }
        Ok(TickOutcome {
            state: Tick::Running,
            notices,
        })
    }

    pub fn reload_config(&mut self) -> Result<Vec<String>> {
        let sample = self.session.now().sample;
        self.core.reload_config(sample)
    }

    /// Stop every stem with an authored fade. Mandatory before dropping a
    /// shared-manager session: dropping kira handles does NOT stop sounds —
    /// the CLI path is only saved by its own manager dying with it.
    pub fn stop_audio(&mut self, fade_ms: f64) {
        self.session.mixer.stop_all(kira::Tween {
            duration: std::time::Duration::from_secs_f64(fade_ms.max(0.0) / 1000.0),
            ..Default::default()
        });
    }

    /// Max pairwise stem skew in seconds, straight from the mixer. Frame-
    /// cadence polling must expect the one-callback-buffer reporting
    /// transient (ADR 0017): re-read before believing a spike.
    pub fn max_stem_skew(&self) -> f64 {
        self.session.mixer.max_stem_skew()
    }

    /// What the shader needs this frame, straight from the conductor and the
    /// chart evaluator — never from judgment records.
    pub fn visual_frame(&self) -> VisualFrame {
        let pos = self.session.now();
        let preroll = pos.sample < 0;
        let beats_per_bar = self.core.beats_per_bar(pos.sample.max(0));
        let target = if preroll {
            [0.0, 0.0]
        } else {
            self.visual_eval
                .sample_channel("lean", pos.beat)
                .and_then(|v| v.as_vec2())
                .unwrap_or([0.0, 0.0])
        };
        VisualFrame {
            beat_phase: (pos.beat.rem_euclid(1.0)) as f32,
            bar_phase: (pos.beat_in_bar / beats_per_bar).clamp(0.0, 1.0) as f32,
            lean: [self.lean[0] as f32, self.lean[1] as f32],
            target: [target[0] as f32, target[1] as f32],
            coherence: self.core.coherence().value() as f32,
            pulse_age_s: self
                .last_pulse_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(1e6),
            preroll,
            desaturate: self.ladder_params.visual.desaturate,
            blur: self.ladder_params.visual.blur,
            chromatic: self.ladder_params.visual.chromatic,
            reassembly: self.reassembly as f32,
            no_input: self.input_rx.is_some() && self.input_events == 0 && !preroll,
            rewind: self.rewind as f32,
        }
    }

    /// Tear down in order: input producer first, then judgment bookkeeping,
    /// then (by drop) the audio handles and any owned manager. On a shared
    /// manager, call [`ChartSession::stop_audio`] first — handles dropping
    /// does not stop sounds. Returns the summary notice lines.
    pub fn finish(mut self) -> Result<Vec<String>> {
        drop(self.input_guard.take()); // stop capture before the audio manager
        let mut notices = Vec::new();
        let s = self.core.finish()?;
        notices.push(format!(
            "done. pulses hit {} (mean |err| {:.1} ms), missed {}, spurious {}",
            s.hits, s.mean_abs_err_ms, s.misses, s.spurious
        ));
        if let Some((mut w, path)) = self.session_writer.take() {
            w.flush()?;
            notices.push(format!(
                "session recorded: {} ({} event(s))",
                path.display(),
                w.written()
            ));
        }
        Ok(notices)
    }
}
