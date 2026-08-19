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
use crate::conductor::{Conductor, MusicalPosition};
use crate::gradient::{GradientConfig, GradientDriver};
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
    /// The error-driven audio gradient (ADR 0024): lean error → wobble depth
    /// + trims, evaluated per tick and summed into the ladder driver's apply.
    gradient: GradientDriver,
    gradient_path: PathBuf,
    records: Vec<JudgmentRecord>,
    seq_events: Vec<ReintegrationEvent>,
    hits: u64,
    misses: u64,
    spurious: u64,
    abs_err_sum_ms: f64,
    /// Second evaluator instance, kept out of the judge for conducted-frame
    /// target lookups (never from judgment records).
    visual_eval: ChartEval,
    /// Latest observed lean / pulse, suite-stamped ([`ChartCore::observe_input`]).
    lean: [f64; 2],
    last_pulse_sample: Option<i64>,
    /// Latest [`SeqTick`] state, cached in [`ChartCore::step_seq`] so the
    /// conducted frame reads one source live and offline.
    ladder_params: LadderParams,
    reassembly: f64,
    rewind: f64,
    /// Pulses judged by the most recent [`ChartCore::process`] call —
    /// cleared at its top, so one frame's worth (ADR 0020).
    frame_pulses: Vec<(i64, f64, PulseKind)>,
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
        gradient: GradientDriver,
        gradient_path: PathBuf,
        visual_eval: ChartEval,
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
            gradient,
            gradient_path,
            records: Vec::new(),
            seq_events: Vec::new(),
            hits: 0,
            misses: 0,
            spurious: 0,
            abs_err_sum_ms: 0.0,
            visual_eval,
            lean: [0.0; 2],
            last_pulse_sample: None,
            ladder_params: LadderParams::clean(),
            reassembly: 1.0,
            rewind: 0.0,
            frame_pulses: Vec::new(),
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

    /// Track lean and last-pulse state from a drained, suite-stamped event.
    /// Call for EVERY event, before any `sample >= 0` ingest gate — preroll
    /// leans still move the world (ADR 0020).
    pub fn observe_input(&mut self, ev: &InputEvent) {
        match ev {
            InputEvent::Lean(l) => self.lean = [l.x, l.y],
            InputEvent::Pulse(p) => self.last_pulse_sample = Some(p.sample),
        }
    }

    /// The latest observed lean (deadzoned, [-1,1]²).
    pub fn lean(&self) -> [f64; 2] {
        self.lean
    }

    /// Expire due windows up to `now_suite`.
    pub fn advance_to(&mut self, now_suite: i64) {
        self.judge.advance_to(now_suite, &mut self.records);
    }

    /// Step coherence over the pending records, tally, and write them to the
    /// judgment log. Returns the pulse errors judged this step (the live
    /// front end echoes each; offline stays quiet).
    pub fn process(&mut self, at_sample: i64) -> Result<Vec<f64>> {
        // One frame's worth of conducted pulses: cleared before the empty-
        // records early return so a quiet frame reports no pulses.
        self.frame_pulses.clear();
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
                JudgmentRecord::Pulse { sample, err_ms, .. } => {
                    self.hits += 1;
                    self.abs_err_sum_ms += err_ms.abs();
                    pulse_errs.push(*err_ms);
                    self.frame_pulses.push((*sample, *err_ms, PulseKind::Hit));
                }
                JudgmentRecord::Miss { sample, .. } => {
                    self.misses += 1;
                    self.frame_pulses.push((*sample, 0.0, PulseKind::Miss));
                }
                JudgmentRecord::Spurious { sample, .. } => {
                    self.spurious += 1;
                    self.frame_pulses.push((*sample, 0.0, PulseKind::Spurious));
                }
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
    /// Call only past pre-roll, after `session.pump()`. `pos` is the frame's
    /// musical position (the caller already has it) — the error gradient
    /// samples the chart's lean target at `pos.beat` (ADR 0024).
    pub fn step_seq(
        &mut self,
        session: &mut SuiteSession,
        pos: &MusicalPosition,
    ) -> Result<(SeqTick, Vec<ReintegrationEvent>)> {
        // The gradient's two inputs, recomputed exactly as the scene binding
        // computes its visual gradient (one param source, one world): the
        // Euclidean distance from the player's lean to the chart's
        // interpolated target, and the lean magnitude for the neutral sink.
        let target = self
            .visual_eval
            .sample_channel("lean", pos.beat)
            .and_then(|v| v.as_vec2())
            .unwrap_or([0.0, 0.0]);
        let err = ((self.lean[0] - target[0]).powi(2) + (self.lean[1] - target[1]).powi(2)).sqrt();
        let lean_mag = (self.lean[0].powi(2) + self.lean[1].powi(2)).sqrt();
        let now_seconds =
            pos.sample.max(0) as f64 / self.conductor.tempo().sample_rate() as f64;
        // Evaluate only while Playing: the seam resets the gradient, and
        // holding the slew still through Failing/Reassembling means play
        // resumes with the gradient easing in from silence, not jumping to
        // a level it accrued while inaudible.
        let offsets = if self.reintegrator.phase() == crate::reintegration::SeqPhase::Playing {
            self.gradient.evaluate(err, lean_mag, now_seconds)
        } else {
            crate::gradient::GradientOffsets::default()
        };

        let seq = self.reintegrator.tick(
            self.coherence.value(),
            &offsets,
            &mut self.ladder,
            &mut self.ladder_driver,
            session,
            &mut self.seq_events,
        )?;
        // Cache the tick's visual state so `conducted_frame` reads one
        // source on both the live and offline paths (ADR 0020).
        self.ladder_params = seq.params.clone();
        self.reassembly = seq.reassembly;
        self.rewind = seq.rewind;
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
                    // The world re-enters clean; the gradient eases back in
                    // from silence via its own attack (mirrors driver.reset).
                    self.gradient.reset();
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
        if self.gradient_path.exists() {
            match GradientConfig::load(&self.gradient_path) {
                Ok(cfg) => {
                    self.gradient.reconfigure(cfg);
                    notices.push("gradient config reloaded (slew state carries over)".to_string());
                    self.log.write_value(&serde_json::json!({
                        "t": "gradient_reload", "sample": sample,
                        "gradient_config": self.gradient.config().to_json(),
                    }))?;
                }
                Err(e) => {
                    notices.push(format!("gradient reload failed, keeping current config: {e}"))
                }
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

    /// The conducted snapshot for this frame (ADR 0020) — from the conductor,
    /// the chart evaluator, and the cached ladder/sequencer state; never from
    /// judgment records except the per-frame pulse buffer. Read-only: safe to
    /// call more than once per tick. All ages are suite-sample arithmetic
    /// (deterministic; they jump across a reintegration seam by design).
    /// `no_input` is live-session context the caller supplies (offline: false).
    pub fn conducted_frame(
        &self,
        pos: crate::conductor::MusicalPosition,
        no_input: bool,
    ) -> ConductedFrame {
        let preroll = pos.sample < 0;
        let beats_per_bar = self.beats_per_bar(pos.sample.max(0));
        let target = if preroll {
            [0.0, 0.0]
        } else {
            self.visual_eval
                .sample_channel("lean", pos.beat)
                .and_then(|v| v.as_vec2())
                .unwrap_or([0.0, 0.0])
        };
        let target = [target[0] as f32, target[1] as f32];
        // Lookahead stays live during preroll (ADR 0023): pos.beat is
        // negative there, so the countdown to the first key telegraphs it.
        let (next_target, next_target_beats) = self
            .visual_eval
            .next_key("lean", pos.beat)
            .and_then(|(b, v)| {
                v.as_vec2()
                    .map(|t| ([t[0] as f32, t[1] as f32], b - pos.beat))
            })
            .unwrap_or((target, 1e6));
        let rate = self.conductor.tempo().sample_rate() as f64;
        let age_s = |sample: i64| ((pos.sample - sample).max(0)) as f64 / rate;
        let section = self
            .conductor
            .section_at_sample(pos.sample.max(0))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        ConductedFrame {
            beat_phase: (pos.beat.rem_euclid(1.0)) as f32,
            bar_phase: (pos.beat_in_bar / beats_per_bar).clamp(0.0, 1.0) as f32,
            lean: [self.lean[0] as f32, self.lean[1] as f32],
            target,
            next_target,
            next_target_beats,
            coherence: self.coherence.value() as f32,
            pulse_age_s: self
                .last_pulse_sample
                .map(|s| age_s(s) as f32)
                .unwrap_or(1e6),
            preroll,
            desaturate: self.ladder_params.visual.desaturate,
            blur: self.ladder_params.visual.blur,
            chromatic: self.ladder_params.visual.chromatic,
            reassembly: self.reassembly as f32,
            no_input,
            rewind: self.rewind as f32,
            bar: pos.bar,
            beat: pos.beat,
            section,
            pulses: self
                .frame_pulses
                .iter()
                .map(|&(sample, err_ms, kind)| ConductedPulse {
                    sample,
                    age_s: age_s(sample),
                    err_ms,
                    kind,
                })
                .collect(),
        }
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
    /// Explicit gradient-config path (ADR 0024; same contract — explicit
    /// must load, default `<base_dir>/config/gradient.toml` is optional,
    /// absent = inert built-ins).
    pub gradient_config: Option<PathBuf>,
    /// Explicit haptics-config path (ADR 0026; same contract — explicit
    /// must load, default `<base_dir>/config/haptics.toml` is optional,
    /// absent = inert built-ins that never emit an event).
    pub haptics_config: Option<PathBuf>,
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

/// How one judged pulse resolved, for scene bindings. Distinct from the
/// chart's verb-space `kind` (which stays "pulse" for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PulseKind {
    Hit,
    Miss,
    Spurious,
}

impl PulseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PulseKind::Hit => "hit",
            PulseKind::Miss => "miss",
            PulseKind::Spurious => "spurious",
        }
    }
}

/// One pulse judged this frame (ADR 0020). `sample` is the suite sample the
/// judgment record carries (hit/spurious: the press; miss: the window close);
/// `age_s` is seconds between that sample and the frame's position.
#[derive(Debug, Clone, PartialEq)]
pub struct ConductedPulse {
    pub sample: i64,
    pub age_s: f64,
    /// Timing error in ms (hits only; 0.0 for miss/spurious).
    pub err_ms: f64,
    pub kind: PulseKind,
}

/// The per-frame conducted-parameters snapshot (ADR 0020): what scene
/// bindings and the harness window draw from. No numbers reach the player's
/// screen — these drive glows, breathing, and world motion, not meters.
#[derive(Debug, Clone, PartialEq)]
pub struct ConductedFrame {
    /// 0..1 within the current beat / bar (0 at the boundary).
    pub beat_phase: f32,
    pub bar_phase: f32,
    /// Player lean (deadzoned) and the chart's lean target, both in [-1,1]².
    pub lean: [f32; 2],
    pub target: [f32; 2],
    /// The next authored lean key's value (ADR 0023) — where the chart bends
    /// next. Falls back to `target` when nothing is upcoming.
    pub next_target: [f32; 2],
    /// Suite beats until that key's anchor (large — 1e6 — when none). Live
    /// during preroll: a shrinking countdown to the first key. Flips to the
    /// following key exactly at each anchor.
    pub next_target_beats: f64,
    pub coherence: f32,
    /// Seconds since the player's most recent pulse press (large if none).
    /// Suite-sample arithmetic — jumps across a reintegration seam by design.
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
    /// Musical position: bar (1-based past preroll), suite beats from zero.
    pub bar: i64,
    pub beat: f64,
    /// Current section name; "" when the manifest has none here.
    pub section: String,
    /// Pulses judged this frame (empty most frames).
    pub pulses: Vec<ConductedPulse>,
}

impl Default for ConductedFrame {
    /// The neutral no-session state: a clean, settled world (coherence and
    /// reassembly at 1.0) so bindings mapping `1 - coherence` show nothing.
    fn default() -> Self {
        Self {
            beat_phase: 0.0,
            bar_phase: 0.0,
            lean: [0.0; 2],
            target: [0.0; 2],
            next_target: [0.0; 2],
            next_target_beats: 1e6,
            coherence: 1.0,
            pulse_age_s: 1e6,
            preroll: false,
            desaturate: 0.0,
            blur: 0.0,
            chromatic: 0.0,
            reassembly: 1.0,
            no_input: false,
            rewind: 0.0,
            bar: 0,
            beat: 0.0,
            section: String::new(),
            pulses: Vec::new(),
        }
    }
}

/// The pre-ADR-0020 name, kept so existing front ends read naturally.
pub type VisualFrame = ConductedFrame;

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
    end_sample: i64,
    sample_rate: u32,
    last_bar: i64,
    /// Total input events received; a live capture that delivers nothing is
    /// the failure mode worth shouting about (ADR 0011).
    input_events: u64,
    warned_no_input: bool,
    /// The haptic decision layer (ADR 0026): pure, event-shaped, live
    /// sessions only — replay and offline renders never construct a
    /// `ChartSession`, so haptics is absent from every deterministic path
    /// by construction.
    haptics: crate::haptics::HapticsDriver,
    haptics_path: PathBuf,
    /// Where haptic events go: the front end's adapter onto the capture
    /// thread's rumble channel. `None` (no rumble hardware, no front-end
    /// wiring) silently drops them.
    haptic_sink: Option<Box<dyn FnMut(crate::haptics::HapticEvent) + Send>>,
    /// Events emitted before the sink attached (the session-start Config).
    haptic_pending: Vec<crate::haptics::HapticEvent>,
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

        // Gradient config (ADR 0024): same contract again — explicit path
        // must load, the default file is optional, absent = inert built-ins
        // (so a gradient-free repo renders byte-identically).
        let gradient_explicit = cfg.gradient_config.is_some();
        let gradient_path = cfg
            .gradient_config
            .clone()
            .unwrap_or_else(|| cfg.base_dir.join("config/gradient.toml"));
        let gradient_cfg = if gradient_explicit || gradient_path.exists() {
            let gcfg = GradientConfig::load(&gradient_path).map_err(|e| {
                FlintError::ValidationError(format!("loading {}: {e}", gradient_path.display()))
            })?;
            notices.push(format!(
                "gradient config: {} (tune bus: {})",
                gradient_path.display(),
                gcfg.tune.bus
            ));
            gcfg
        } else {
            notices.push(format!(
                "gradient config: inert built-ins (no {})",
                gradient_path.display()
            ));
            GradientConfig::default()
        };
        let gradient = GradientDriver::new(gradient_cfg);
        // A silent tune bus makes the wobble a no-op — true of the current
        // placeholder track's world_voice (per-friend stems are queued music
        // work). Loud, not silent: the ADR 0011 lesson.
        if gradient.config().is_active() {
            let tune_bus = &gradient.config().tune.bus;
            let playing = session
                .mixer
                .buses()
                .find(|b| b.name == *tune_bus)
                .is_some_and(|b| b.state.playing);
            if !playing {
                notices.push(format!(
                    "  *** gradient tune bus '{tune_bus}' is SILENT in this manifest — \
                     the wobble is inaudible here (sink trims still apply)"
                ));
            }
        }

        // Haptics config (ADR 0026): the same contract one more time —
        // explicit path must load, the default file is optional, absent =
        // inert built-ins (no events, ever; motors need no protecting the
        // way buses do, but silence-by-default keeps the feature opt-in).
        let haptics_explicit = cfg.haptics_config.is_some();
        let haptics_path = cfg
            .haptics_config
            .clone()
            .unwrap_or_else(|| cfg.base_dir.join("config/haptics.toml"));
        let haptics_cfg = if haptics_explicit || haptics_path.exists() {
            let hcfg = crate::haptics::HapticsConfig::load(&haptics_path).map_err(|e| {
                FlintError::ValidationError(format!("loading {}: {e}", haptics_path.display()))
            })?;
            notices.push(format!(
                "haptics config: {} (tick grid: {}, lead {:.0} ms)",
                haptics_path.display(),
                hcfg.tick_grid.as_str(),
                hcfg.lead_ms
            ));
            hcfg
        } else {
            notices.push(format!(
                "haptics config: inert built-ins (no {})",
                haptics_path.display()
            ));
            crate::haptics::HapticsConfig::default()
        };
        let haptics = crate::haptics::HapticsDriver::new(haptics_cfg);

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
            "gradient_config": gradient.config().to_json(),
            "haptics_config": haptics.config().to_json(),
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
            gradient,
            gradient_path,
            visual_eval,
        );
        core.sync_seam_params();

        // The session-start engine settings ride the pending queue until a
        // front end attaches a sink; an inert config never emits at all.
        let haptic_pending = if haptics.config().is_active() {
            vec![haptics.config_event(sample_rate)]
        } else {
            Vec::new()
        };

        Ok((
            Self {
                core,
                session,
                session_writer,
                bridge,
                input_guard: None,
                input_rx: None,
                end_sample,
                sample_rate,
                last_bar: i64::MIN,
                input_events: 0,
                warned_no_input: false,
                haptics,
                haptics_path,
                haptic_sink: None,
                haptic_pending,
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

    /// Attach the haptic-event sink (ADR 0026): the front end's adapter onto
    /// the capture thread's rumble channel. Events arrive suite-addressed
    /// converted to **raw clock samples** already — the sink forwards them
    /// verbatim. Without a sink the driver never runs.
    pub fn set_haptic_sink(
        &mut self,
        sink: Box<dyn FnMut(crate::haptics::HapticEvent) + Send>,
    ) {
        self.haptic_sink = Some(sink);
    }

    /// Whether the resolved haptics config can emit anything — front ends
    /// use this to skip acquiring rumble hardware for inert sessions.
    pub fn haptics_active(&self) -> bool {
        self.haptics.config().is_active()
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
                // Suite-stamped so conducted pulse ages are suite-time
                // arithmetic; before the gate so preroll leans still move.
                self.core.observe_input(&suite_ev);
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
        // The core caches the SeqTick state for `conducted_frame`.
        let (seq, seq_events) = self.core.step_seq(&mut self.session, &pos)?;

        // Haptics (ADR 0026): pure decision layer over this tick's events,
        // forwarded through the front end's sink onto the rumble channel.
        // Suite→raw conversion happens here — the capture thread speaks the
        // bridge's raw clock and never learns about seams beyond Flush.
        // Suite positions are latency-compensated EAR time (`session.now()`
        // subtracts output latency), so the raw-clock moment the ear hears
        // suite sample S is S + timeline_offset + latency_samples — both
        // terms, or every burst lands ~latency early and an "immediate"
        // burst reads ~latency LATE and is drop-gated into silence (the
        // bug behind the first silent feel run).
        if let Some(sink) = &mut self.haptic_sink {
            let mut events = std::mem::take(&mut self.haptic_pending);
            self.haptics.evaluate(
                &self.session.conductor,
                pos.sample,
                self.core.reintegrator.phase(),
                seq.rewind,
                self.core.reintegrator.pickup_beats,
                &self.core.frame_pulses,
                &seq_events,
                &mut events,
            );
            let offset =
                self.session.timeline_offset() + self.session.conductor.latency_samples();
            for ev in events {
                sink(match ev {
                    crate::haptics::HapticEvent::Burst {
                        at_suite_sample,
                        strong,
                        weak,
                        duration_samples,
                    } => crate::haptics::HapticEvent::Burst {
                        at_suite_sample: at_suite_sample + offset,
                        strong,
                        weak,
                        duration_samples,
                    },
                    other => other,
                });
            }
        }

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
                self.core.lean()[0],
                self.core.lean()[1],
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
        let mut notices = self.core.reload_config(sample)?;
        // Haptics reloads at the session level — the driver lives here, not
        // in the core (replay/offline never carry one). Same contract:
        // reload only when the file exists, failure keeps the current
        // config, success re-sends the engine settings (lead changes must
        // reach already-scheduled bursts).
        if self.haptics_path.exists() {
            match crate::haptics::HapticsConfig::load(&self.haptics_path) {
                Ok(cfg) => {
                    self.haptics.reconfigure(cfg);
                    notices.push("haptics config reloaded".to_string());
                    self.core.log.write_value(&serde_json::json!({
                        "t": "haptics_reload", "sample": sample,
                        "haptics_config": self.haptics.config().to_json(),
                    }))?;
                    let ev = self.haptics.config_event(self.sample_rate);
                    match &mut self.haptic_sink {
                        Some(sink) => sink(ev),
                        None => self.haptic_pending.push(ev),
                    }
                }
                Err(e) => {
                    notices.push(format!("haptics reload failed, keeping current config: {e}"))
                }
            }
        }
        Ok(notices)
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

    /// What the shader and scene bindings need this frame, straight from the
    /// conductor and the chart evaluator — never from judgment records
    /// (except the per-frame pulse buffer, ADR 0020).
    pub fn visual_frame(&self) -> ConductedFrame {
        let pos = self.session.now();
        let no_input = self.input_rx.is_some() && self.input_events == 0 && pos.sample >= 0;
        self.core.conducted_frame(pos, no_input)
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
