//! Play a suite against its beatmap chart with live input capture: the
//! Phase 2 dev harness. Validates manifest and chart, starts every bus
//! sample-locked, spawns the 1 kHz capture thread, and shows the shared
//! status readout. Judgment, coherence, and session recording hang off this
//! loop as they land. Dev-only surface — never part of a player-facing build.
//!
//! Two front ends share one [`ChartSession`]: the console loop below, and
//! the `--window` mode in `play_chart_window` (a focused window keeps
//! gamepad-to-keyboard mappers from typing into the terminal, and carries
//! the wordless visual cues).

use anyhow::{bail, Context, Result};
use flint_music::chart_eval::ChartEval;
use flint_music::clock_bridge::ClockBridge;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::input_stream::InputEvent;
use flint_music::judgment::{Judge, JudgmentConfig, JudgmentRecord, JsonlWriter, LeanMode};
use flint_music::ladder::{Ladder, LadderConfig, LadderDriver, LadderParams};
use flint_music::reintegration::{ReintegrationEvent, Reintegrator};
use flint_music::session::RealtimePlayer;
use flint_music::status::status_line_with_coherence;
use flint_music::{validate_chart, validate_manifest, validate_manifest_assets, Chart, SuiteManifest};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::latency_files::{latest_calibration_ms, latest_latency_ms};

pub struct PlayChartArgs {
    pub manifest: String,
    pub chart: String,
    pub base_dir: Option<String>,
    pub bars: Option<u64>,
    /// Coherence config path (default: `<base_dir>/config/coherence.toml`
    /// when present, else built-in defaults).
    pub config: Option<String>,
    /// Record the input session to `logs/sessions/<name>.session.jsonl`.
    pub record: Option<String>,
    /// Run the input-granularity spike for this many seconds and exit
    /// (wiggle the stick; no audio involved).
    pub spike_input_secs: Option<u64>,
    /// Open a bare graphical window instead of running console-only.
    pub window: bool,
    /// Lean judgment mode: "arrival" (beat-anchored targets, the current
    /// feel direction) or "track" (continuous curve-following).
    pub lean_mode: String,
    /// Disintegration ladder config path (default: `<base_dir>/config/
    /// ladder.toml` when present, else built-in defaults).
    pub ladder: Option<String>,
}

/// Shared by play-chart and replay-chart so live and offline judge alike.
pub fn parse_lean_mode(s: &str) -> Result<LeanMode> {
    match s {
        "arrival" => Ok(LeanMode::Arrival),
        "track" => Ok(LeanMode::Track),
        other => bail!("unknown --lean-mode `{other}` (expected `arrival` or `track`)"),
    }
}

pub fn lean_mode_name(mode: LeanMode) -> &'static str {
    match mode {
        LeanMode::Arrival => "arrival",
        LeanMode::Track => "track",
    }
}

pub fn run(args: PlayChartArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    if let Some(secs) = args.spike_input_secs {
        return run_spike(&base_dir, secs);
    }

    let session = ChartSession::open(&args, &base_dir)?;
    if args.window {
        super::play_chart_window::run_windowed(session)
    } else {
        run_cli(session)
    }
}

/// Everything one play-through owns, front-end agnostic. A front end calls
/// [`ChartSession::tick`] at its own cadence (the audio clock is the master
/// clock — ticking merely drains queues and advances judgment), plus
/// [`ChartSession::reload_config`] / [`ChartSession::finish`] on its own
/// input gestures.
pub struct ChartSession {
    player: RealtimePlayer,
    judge: Judge,
    coherence: Coherence,
    judgment_cfg: JudgmentConfig,
    /// Musical time per lean record, precomputed for coherence's integrator.
    lean_step_beats: f64,
    log: JsonlWriter,
    session_writer: Option<(flint_music::replay::SessionWriter, PathBuf)>,
    bridge: ClockBridge,
    capture_handle: Option<flint_input_capture::CaptureHandle>,
    input_rx: Option<std::sync::mpsc::Receiver<InputEvent>>,
    config_path: PathBuf,
    ladder: Ladder,
    ladder_driver: LadderDriver,
    ladder_path: PathBuf,
    /// Params currently applied — the visual half feeds `visual_frame`.
    ladder_params: LadderParams,
    reintegrator: Reintegrator,
    /// Reassembly progress for visuals (1.0 in normal play).
    reassembly: f64,
    /// Rewind interlude progress for visuals (0 outside the interlude).
    rewind: f64,
    seq_events: Vec<ReintegrationEvent>,
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
    records: Vec<JudgmentRecord>,
    hits: u64,
    misses: u64,
    spurious: u64,
    abs_err_sum_ms: f64,
    last_pulse_at: Option<Instant>,
}

#[derive(PartialEq)]
pub enum Tick {
    Running,
    Finished,
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

impl ChartSession {
    pub fn open(args: &PlayChartArgs, base_dir: &Path) -> Result<Self> {
        // --- validate both files first --------------------------------------
        let manifest = SuiteManifest::load(Path::new(&args.manifest))
            .with_context(|| format!("loading {}", args.manifest))?;
        let chart = Chart::load(Path::new(&args.chart))
            .with_context(|| format!("loading {}", args.chart))?;
        let mut issues = validate_manifest(&manifest);
        issues.extend(validate_manifest_assets(&manifest, base_dir));
        issues.extend(validate_chart(&chart, &manifest));
        if !issues.is_empty() {
            for i in &issues {
                eprintln!("{i}");
            }
            bail!("validation failed ({} issue(s)); not playing", issues.len());
        }

        // --- offsets ---------------------------------------------------------
        let latency_ms = match latest_latency_ms(base_dir) {
            Some((file, ms)) => {
                println!("measured output latency: {ms:.1} ms (compensating; from {file})");
                Some(ms)
            }
            None => {
                println!(
                    "measured output latency: NONE ON RECORD — run spikes/latency_harness \
                     and commit its log to logs/latency/"
                );
                None
            }
        };
        let calibration_ms = match latest_calibration_ms(base_dir) {
            Some((file, ms)) => {
                println!("tap calibration: {ms:+.1} ms (from {file})");
                ms
            }
            None => {
                println!("tap calibration: none on record (run `flint calibrate`)");
                0.0
            }
        };
        // Total judgment offset: what the ear hears, adjusted by the player's
        // own measured tap tendency.
        let offset_samples = ((latency_ms.unwrap_or(0.0) + calibration_ms) / 1000.0
            * manifest.sample_rate as f64)
            .round() as i64;

        // --- session + judgment ----------------------------------------------
        let player = RealtimePlayer::start(&manifest, base_dir, latency_ms)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // Judgment arithmetic is pure: event samples arrive already compensated
        // (capture subtracts the offset), so its conductor carries none.
        let judge_conductor = Conductor::new(&manifest, None);
        let visual_eval =
            ChartEval::new(&chart, &judge_conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
        let eval = ChartEval::new(&chart, &judge_conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
        let judgment_cfg = JudgmentConfig {
            lean_mode: parse_lean_mode(&args.lean_mode)?,
            ..Default::default()
        };
        println!("lean mode: {}", lean_mode_name(judgment_cfg.lean_mode));
        let judge = Judge::new(eval, judge_conductor, judgment_cfg);
        let lean_step_beats = judge.lean_step_beats();

        // Coherence config: explicit path must load; the default path is
        // optional. `r`+Enter (or `r` in the window) reloads it mid-session.
        let config_explicit = args.config.is_some();
        let config_path = args
            .config
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| base_dir.join("config/coherence.toml"));
        let config_required = config_explicit || config_path.exists();
        let coherence_cfg = if config_required {
            let cfg = CoherenceConfig::load(&config_path)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| format!("loading {}", config_path.display()))?;
            println!(
                "coherence config: {} (r+Enter = reload, q+Enter = quit)",
                config_path.display()
            );
            cfg
        } else {
            println!("coherence config: built-in defaults (no {})", config_path.display());
            CoherenceConfig::default()
        };
        let coherence = Coherence::new(coherence_cfg);

        // Ladder config: same contract as the coherence config — explicit
        // path must load, default path is optional, reloads with `r`.
        let ladder_explicit = args.ladder.is_some();
        let ladder_path = args
            .ladder
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| base_dir.join("config/ladder.toml"));
        let ladder_cfg = if ladder_explicit || ladder_path.exists() {
            let cfg = LadderConfig::load(&ladder_path)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| format!("loading {}", ladder_path.display()))?;
            println!("ladder config: {} ({} rung(s))", ladder_path.display(), cfg.rungs.len());
            cfg
        } else {
            println!("ladder config: built-in defaults (no {})", ladder_path.display());
            LadderConfig::default()
        };
        let ladder = Ladder::new(ladder_cfg);
        let mut reintegrator = Reintegrator::new(manifest.reintegration.clone());
        reintegrator.fade_ms = ladder.config().seam_fade_ms;
        reintegrator.rewind_bars = ladder.config().seam_rewind_bars;
        reintegrator.rewind_drop_st = ladder.config().seam_rewind_drop_st;

        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log_path = base_dir.join(format!("logs/judgment/judgment-{epoch}.jsonl"));
        let header = serde_json::json!({
            "t": "header", "schema": 0,
            "suite": manifest.id, "chart": args.chart,
            "latency_ms": latency_ms.unwrap_or(0.0), "calibration_ms": calibration_ms,
            "grid_beats": judgment_cfg.grid_beats,
            "lean_mode": lean_mode_name(judgment_cfg.lean_mode),
            "arrival_half_beats": judgment_cfg.arrival_half_beats,
            "coherence_config": coherence_cfg.to_json(),
            "ladder_config": ladder.config().to_json(),
            "epoch_s": epoch,
        });
        let log = JsonlWriter::create(&log_path, &header).map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("judgment log: {}", log_path.display());

        let session_writer = match &args.record {
            Some(name) => {
                let path = base_dir.join(format!("logs/sessions/{name}.session.jsonl"));
                let session_header = flint_music::replay::SessionHeader {
                    schema: 0,
                    suite: manifest.id.clone(),
                    chart: args.chart.clone(),
                    sample_rate: manifest.sample_rate,
                    latency_ms: latency_ms.unwrap_or(0.0),
                    calibration_ms,
                    coherence_config: Some(coherence_cfg.to_json()),
                    capture: Some(serde_json::json!({
                        "poll_hz": 1000, "deadzone": 0.12,
                    })),
                };
                let w = flint_music::replay::SessionWriter::create(&path, &session_header)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("recording session: {}", path.display());
                Some((w, path))
            }
            None => None,
        };

        let bridge = ClockBridge::new(manifest.sample_rate);
        let capture = flint_input_capture::spawn(
            bridge.clone(),
            flint_input_capture::CaptureConfig {
                offset_samples,
                ..Default::default()
            },
        );
        let (capture_handle, input_rx) = match capture {
            Ok(pair) => {
                println!("gamepad capture running (left stick = lean, South/RT = pulse)");
                // The backend can be alive with zero devices (ADR 0011's
                // silent failure — e.g. a mapper presenting the pad as
                // keyboard/mouse). Make that loud BEFORE the session.
                match flint_input_capture::connected_gamepads() {
                    Ok(pads) if pads.is_empty() => {
                        println!(
                            "  *** WARNING: NO GAMEPADS VISIBLE — the session will receive no \
                             input.\n  *** On Windows only XInput-class devices are seen; check \
                             the controller/mapper mode (e.g. Legion Space)."
                        );
                    }
                    Ok(pads) => println!("  gamepad(s): {}", pads.join(", ")),
                    Err(_) => {}
                }
                (Some(pair.0), Some(pair.1))
            }
            Err(e) => {
                println!("gamepad capture unavailable ({e}); playing without input");
                (None, None)
            }
        };

        let end_sample = args
            .bars
            .map(|b| player.session.conductor.sample_at_bar(b as i64, 0.0))
            .unwrap_or(i64::MAX);
        let sample_rate = manifest.sample_rate;

        Ok(Self {
            player,
            judge,
            coherence,
            judgment_cfg,
            lean_step_beats,
            log,
            session_writer,
            bridge,
            capture_handle,
            input_rx,
            config_path,
            ladder,
            ladder_driver: LadderDriver::new(),
            ladder_path,
            ladder_params: LadderParams::clean(),
            reintegrator,
            reassembly: 1.0,
            rewind: 0.0,
            seq_events: Vec::new(),
            end_sample,
            sample_rate,
            visual_eval,
            last_bar: i64::MIN,
            input_events: 0,
            warned_no_input: false,
            lean: [0.0; 2],
            records: Vec::new(),
            hits: 0,
            misses: 0,
            spurious: 0,
            abs_err_sum_ms: 0.0,
            last_pulse_at: None,
        })
    }

    /// One iteration: pump the scheduler, feed the clock bridge, drain input
    /// into judgment, step coherence, log, and print the per-bar status line.
    pub fn tick(&mut self) -> Result<Tick> {
        self.player.session.pump();
        // The bridge fits a line through clock observations — it must see the
        // monotonic RAW clock; a seam's backwards jump would poison the fit.
        // Events therefore arrive stamped in raw samples and are re-mapped to
        // suite time at ingest.
        self.bridge
            .observe(Instant::now(), self.player.session.raw_clock_sample());

        let pos = self.player.session.now();

        let timeline_offset = self.player.session.timeline_offset();
        if let Some(rx) = &self.input_rx {
            while let Ok(ev) = rx.try_recv() {
                self.input_events += 1;
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
                        w.write(&ev, self.sample_rate)
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }
                let suite_ev = if timeline_offset != 0 {
                    ev.with_sample(ev.sample() - timeline_offset)
                } else {
                    ev
                };
                if suite_ev.sample() >= 0 {
                    self.judge.ingest(&suite_ev, &mut self.records);
                }
            }
        }
        if pos.sample >= 0 {
            self.judge.advance_to(pos.sample, &mut self.records);
        }
        if !self.records.is_empty() {
            let beats_per_bar = self.beats_per_bar(pos.sample.max(0));
            let value =
                self.coherence
                    .step(&self.records, self.lean_step_beats, beats_per_bar);
            self.log
                .write_value(&serde_json::json!({
                    "t": "coherence", "sample": pos.sample, "value": value,
                }))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        for rec in self.records.drain(..) {
            match &rec {
                JudgmentRecord::Pulse { err_ms, .. } => {
                    self.hits += 1;
                    self.abs_err_sum_ms += err_ms.abs();
                    println!("  pulse {err_ms:+.1} ms");
                }
                JudgmentRecord::Miss { .. } => self.misses += 1,
                JudgmentRecord::Spurious { .. } => self.spurious += 1,
                JudgmentRecord::Track { .. } => {}
            }
            self.log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        if pos.sample < 0 {
            return Ok(Tick::Running); // pre-roll
        }

        // Disintegration ladder + reintegration sequencer: coherence in,
        // mixer tweens + visual params out. Idle most ticks (hysteresis).
        let seq = self
            .reintegrator
            .tick(
                self.coherence.value(),
                &mut self.ladder,
                &mut self.ladder_driver,
                &mut self.player.session,
                &mut self.seq_events,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.ladder_params = seq.params;
        self.reassembly = seq.reassembly;
        self.rewind = seq.rewind;
        for ev in std::mem::take(&mut self.seq_events) {
            match &ev {
                ReintegrationEvent::FullFail {
                    at_suite_sample,
                    re_entry_sample,
                    seam_suite_sample,
                } => {
                    println!(
                        "  FULL FAIL at sample {at_suite_sample}: reintegrating to sample \
                         {re_entry_sample} (seam at {seam_suite_sample})"
                    );
                    self.log
                        .write_value(&serde_json::json!({
                            "t": "full_fail", "sample": at_suite_sample,
                            "re_entry_sample": re_entry_sample,
                            "seam_suite_sample": seam_suite_sample,
                        }))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                ReintegrationEvent::Seam {
                    raw_sample,
                    timeline_offset,
                    re_entry_sample,
                } => {
                    println!("  seam: world re-gathering from sample {re_entry_sample}");
                    self.judge.rewind_to(*re_entry_sample);
                    if let Some((w, _)) = &mut self.session_writer {
                        w.write_timeline_jump(*raw_sample, *timeline_offset)
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                    self.log
                        .write_value(&serde_json::json!({
                            "t": "seam", "raw_sample": raw_sample,
                            "timeline_offset": timeline_offset,
                            "re_entry_sample": re_entry_sample,
                        }))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                ReintegrationEvent::ReassemblyComplete { at_suite_sample } => {
                    println!("  reassembly complete");
                    self.log
                        .write_value(&serde_json::json!({
                            "t": "reassembly_complete", "sample": at_suite_sample,
                        }))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
        }

        if pos.sample >= self.end_sample || self.player.session.mixer.all_stopped() {
            return Ok(Tick::Finished);
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
                println!(
                    "  *** NO INPUT RECEIVED after {} bars — capture is running but the \
                     controller is delivering nothing.\n  *** Check the controller/mapper \
                     mode (XInput), then restart the session.",
                    pos.bar
                );
            }
            let section = self.player.session.conductor.section_at_sample(pos.sample);
            let no_input = if self.input_rx.is_some() && self.input_events == 0 {
                " | NO INPUT"
            } else {
                ""
            };
            let rung = match self.ladder.rung() {
                Some(r) => format!(" | rung {} ({})", self.ladder.level(), r.name),
                None => String::new(),
            };
            println!(
                "{} | lean ({:+.2},{:+.2}){rung}{no_input}",
                status_line_with_coherence(
                    &pos,
                    section,
                    &self.player.session.mixer,
                    self.coherence.value()
                ),
                self.lean[0],
                self.lean[1],
            );
            self.log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(Tick::Running)
    }

    pub fn reload_config(&mut self) -> Result<()> {
        match CoherenceConfig::load(&self.config_path) {
            Ok(cfg) => {
                self.coherence.reconfigure(cfg);
                println!("coherence config reloaded (value carries over)");
                let sample = self.player.session.now().sample;
                self.log
                    .write_value(&serde_json::json!({
                        "t": "config_reload", "sample": sample,
                        "coherence_config": cfg.to_json(),
                    }))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            Err(e) => println!("reload failed, keeping current config: {e}"),
        }
        if self.ladder_path.exists() {
            match LadderConfig::load(&self.ladder_path) {
                Ok(cfg) => {
                    self.ladder.reconfigure(cfg);
                    self.reintegrator.fade_ms = self.ladder.config().seam_fade_ms;
                    self.reintegrator.rewind_bars = self.ladder.config().seam_rewind_bars;
                    self.reintegrator.rewind_drop_st = self.ladder.config().seam_rewind_drop_st;
                    println!("ladder config reloaded (level carries over)");
                    let sample = self.player.session.now().sample;
                    self.log
                        .write_value(&serde_json::json!({
                            "t": "ladder_reload", "sample": sample,
                            "ladder_config": self.ladder.config().to_json(),
                        }))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                Err(e) => println!("ladder reload failed, keeping current config: {e}"),
            }
        }
        Ok(())
    }

    /// What the shader needs this frame, straight from the conductor and the
    /// chart evaluator — never from judgment records.
    pub fn visual_frame(&self) -> VisualFrame {
        let pos = self.player.session.now();
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
        VisualFrame {
            beat_phase: (pos.beat.rem_euclid(1.0)) as f32,
            bar_phase: (pos.beat_in_bar / beats_per_bar).clamp(0.0, 1.0) as f32,
            lean: [self.lean[0] as f32, self.lean[1] as f32],
            target: [target[0] as f32, target[1] as f32],
            coherence: self.coherence.value() as f32,
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

    pub fn finish(mut self) -> Result<()> {
        drop(self.capture_handle.take()); // stop capture before the audio manager
        self.judge.finish(&mut self.records);
        for rec in self.records.drain(..) {
            match &rec {
                JudgmentRecord::Miss { .. } => self.misses += 1,
                JudgmentRecord::Spurious { .. } => self.spurious += 1,
                _ => {}
            }
            self.log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        self.log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mean_ms = if self.hits > 0 {
            self.abs_err_sum_ms / self.hits as f64
        } else {
            0.0
        };
        println!(
            "done. pulses hit {} (mean |err| {mean_ms:.1} ms), missed {}, spurious {}",
            self.hits, self.misses, self.spurious
        );
        if let Some((mut w, path)) = self.session_writer.take() {
            w.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("session recorded: {} ({} event(s))", path.display(), w.written());
        }
        Ok(())
    }

    fn beats_per_bar(&self, sample: i64) -> f64 {
        self.player
            .session
            .conductor
            .tempo()
            .anchor_at(sample)
            .map(|a| a.beats_per_bar as f64)
            .unwrap_or(4.0)
    }
}

/// Console front end: 2 ms cadence, line-buffered stdin for control.
fn run_cli(mut session: ChartSession) -> Result<()> {
    let stdin_rx = spawn_stdin_reader();
    loop {
        std::thread::sleep(Duration::from_millis(2));
        if session.tick()? == Tick::Finished {
            break;
        }
        match stdin_rx.try_recv() {
            Ok(StdinCommand::Quit) => {
                println!("quit requested");
                break;
            }
            Ok(StdinCommand::Reload) => session.reload_config()?,
            Err(_) => {}
        }
    }
    session.finish()
}

fn run_spike(base_dir: &Path, secs: u64) -> Result<()> {
    let pads = flint_input_capture::connected_gamepads().map_err(|e| anyhow::anyhow!("{e}"))?;
    if pads.is_empty() {
        eprintln!(
            "WARNING: the input backend sees NO gamepads — the spike will record zero \
             events.\nOn Windows this build uses the XInput backend; DirectInput-only \
             pads (e.g. DualShock/DualSense without a mapper) are invisible to it."
        );
    } else {
        println!("gamepad(s) visible: {}", pads.join(", "));
    }
    println!("input-granularity spike: wiggle the stick for {secs} s...");
    let report =
        flint_input_capture::measure_granularity(Duration::from_secs(secs), 1000)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "{} events; receipt median {:.3} ms, driver median {:.3} ms",
        report.events, report.receipt_median_ms, report.driver_median_ms
    );
    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).context("creating logs/latency")?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("input-granularity-{host}-{epoch}.toml"));
    std::fs::write(&path, report.to_toml()).with_context(|| format!("writing {path:?}"))?;
    println!("wrote {}", path.display());
    Ok(())
}

enum StdinCommand {
    Reload,
    Quit,
}

/// Line-buffered stdin on its own thread: `r` + Enter reloads the coherence
/// config, `q` + Enter quits, anything else (including bare Enter) is ignored
/// — gamepad-to-keyboard mappers (Steam's desktop layout maps A → Enter) spam
/// stray keystrokes into the console during play, so every command needs a
/// deliberate letter. Line-buffered beats raw mode here — no extra deps, no
/// terminal-state cleanup, and a dev harness doesn't need better.
fn spawn_stdin_reader() -> std::sync::mpsc::Receiver<StdinCommand> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("play-chart-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                use std::io::BufRead;
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    return; // EOF (piped/headless): no interactive control
                }
                let cmd = match line.trim() {
                    "q" | "quit" => Some(StdinCommand::Quit),
                    "r" | "reload" => Some(StdinCommand::Reload),
                    _ => None, // stray input (gamepad mappers, accidental Enter)
                };
                if let Some(cmd) = cmd {
                    if tx.send(cmd).is_err() {
                        return;
                    }
                }
            }
        })
        .ok();
    rx
}
