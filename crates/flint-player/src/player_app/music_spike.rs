//! THROWAWAY — Phase 4 F2 player-integration spike. Delete when F3's
//! `music_session` lifecycle lands.
//!
//! Proves the two structural fights in isolation (ADR 0017 / ADR 0018):
//! the prototype suite started on the player's shared kira `AudioManager`
//! (spatial SFX and six sample-locked stems on one device stream), and the
//! gamepad handoff (the 1 kHz capture thread owns the pad; the player
//! consumes its channel, feeding the Judge at full precision and
//! down-sampling the same drain into `InputState`).
//!
//! Evidence goes to `logs/spike/f2-<epoch>.jsonl` plus stdout: per-second
//! stem skew (with and without a concurrent spatial one-shot), per-pulse
//! judgment error, input granularity and stamp-monotonicity stats.

use anyhow::{anyhow, Context, Result};
use flint_audio::engine::AudioEngine;
use flint_core::Vec3;
use flint_input_capture::{CaptureConfig, CaptureHandle};
use flint_music::chart_session::{latest_calibration_ms, latest_latency_ms};
use flint_music::judgment::{JsonlWriter, Judge, JudgmentConfig, JudgmentRecord, LeanMode};
use flint_music::session::FileStems;
use flint_music::{
    validate_chart, validate_manifest, validate_manifest_assets, Chart, ChartEval, ClockBridge,
    Conductor, InputEvent, SuiteManifest, SuiteSession,
};
use flint_runtime::InputState;
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

/// Gamepad slot the down-sampled events claim in `InputState`.
const SPIKE_GAMEPAD: u32 = 0;
/// Cadence of the contention one-shot.
const SFX_PERIOD_S: f64 = 2.0;
/// Stop after this many bars so a run stays short.
const END_BARS: i64 = 16;

pub struct MusicSpike {
    // `SuiteSession` holds only kira handles; the manager stays inside
    // `AudioEngine`. PlayerApp declares the spike field before `audio` so
    // these handles drop first (ADR 0017 drop-order rule).
    session: SuiteSession,
    judge: Judge,
    records: Vec<JudgmentRecord>,
    bridge: ClockBridge,
    rx: Option<mpsc::Receiver<InputEvent>>,
    _capture: Option<CaptureHandle>,
    evidence: JsonlWriter,
    sfx_loaded: bool,
    end_sample: i64,
    sample_rate: u32,

    // Evidence bookkeeping.
    started: Instant,
    last_skew_report: Instant,
    last_sfx: Option<Instant>,
    sfx_fired: u64,
    sfx_failed: u64,
    max_skew_ms: f64,
    events_total: u64,
    monotonic_violations: u64,
    last_event_sample: i64,
    last_lean_sample: Option<i64>,
    lean_deltas_ms: Vec<f64>,
    hits: u64,
    misses: u64,
    spurious: u64,
    abs_err_sum_ms: f64,
    pulse_release_pending: bool,
    finished: bool,
}

impl MusicSpike {
    /// Start the suite on the player's shared manager and spawn capture.
    /// `Ok(None)` when no audio device exists (headless — spike skipped,
    /// matching the engine's Option-audio convention).
    pub fn start(base_dir: &Path, audio: &mut AudioEngine) -> Result<Option<Self>> {
        let manifest_path = base_dir.join("assets/manifests/prototype.suite.toml");
        let chart_path = base_dir.join("assets/charts/prototype.chart.toml");

        // --- timing offsets (committed logs in <base_dir>/logs/latency/) ---
        let latency_ms = match latest_latency_ms(base_dir) {
            Some((file, ms)) => {
                println!("[music-spike] measured output latency: {ms:.1} ms (from {file})");
                Some(ms)
            }
            None => {
                println!("[music-spike] measured output latency: NONE ON RECORD");
                None
            }
        };
        let calibration_ms = match latest_calibration_ms(base_dir) {
            Some((file, ms)) => {
                println!("[music-spike] tap calibration: {ms:+.1} ms (from {file})");
                ms
            }
            None => {
                println!("[music-spike] tap calibration: none on record");
                0.0
            }
        };

        // --- validate manifest + chart (same gate as the CLI harness) ------
        let manifest = SuiteManifest::load(&manifest_path)
            .map_err(|e| anyhow!("loading {}: {e}", manifest_path.display()))?;
        let chart = Chart::load(&chart_path)
            .map_err(|e| anyhow!("loading {}: {e}", chart_path.display()))?;
        let mut issues = validate_manifest(&manifest);
        issues.extend(validate_manifest_assets(&manifest, base_dir));
        issues.extend(validate_chart(&chart, &manifest));
        if !issues.is_empty() {
            for i in &issues {
                eprintln!("[music-spike] {i}");
            }
            return Err(anyhow!(
                "suite/chart validation failed ({} issue(s))",
                issues.len()
            ));
        }

        // --- the shared manager (decision 1) -------------------------------
        let Some(manager) = audio.manager_mut() else {
            println!("[music-spike] no audio device — spike skipped");
            return Ok(None);
        };
        let session = SuiteSession::start(
            &manifest,
            manager,
            &FileStems::new(base_dir),
            latency_ms,
        )
        .map_err(|e| anyhow!("starting suite on shared manager: {e}"))?;

        // --- judgment (arrival mode, as the CLI default) -------------------
        let judge_conductor = Conductor::new(&manifest, None);
        let eval = ChartEval::new(&chart, &judge_conductor).map_err(|e| anyhow!("{e}"))?;
        let judgment_cfg = JudgmentConfig {
            lean_mode: LeanMode::Arrival,
            ..Default::default()
        };
        let judge = Judge::new(eval, judge_conductor, judgment_cfg);

        // --- capture handoff (decision 2) ----------------------------------
        let bridge = ClockBridge::new(manifest.sample_rate);
        let offset_samples = ((latency_ms.unwrap_or(0.0) + calibration_ms) / 1000.0
            * manifest.sample_rate as f64)
            .round() as i64;
        let (capture, rx) = match flint_input_capture::spawn(
            bridge.clone(),
            CaptureConfig {
                offset_samples,
                ..Default::default()
            },
        ) {
            Ok((handle, rx)) => {
                println!("[music-spike] gamepad capture running (left stick = lean, South/RT = pulse)");
                match flint_input_capture::connected_gamepads() {
                    Ok(pads) if pads.is_empty() => println!(
                        "[music-spike] *** WARNING: NO GAMEPADS VISIBLE — the spike will \
                         receive no input. On Windows only XInput-class devices are seen; \
                         check the controller/mapper mode."
                    ),
                    Ok(pads) => println!("[music-spike] gamepad(s): {}", pads.join(", ")),
                    Err(_) => {}
                }
                (Some(handle), Some(rx))
            }
            Err(e) => {
                eprintln!("[music-spike] gamepad capture unavailable ({e}) — input criteria will FAIL");
                (None, None)
            }
        };

        // --- contention SFX (cwd = engine\, ogg because kira has no wav) ---
        let sfx_loaded = match audio.load_sound("spike_sfx", Path::new("demo/audio/glasses_clink.ogg"))
        {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[music-spike] contention SFX failed to load ({e}) — contention test disabled");
                false
            }
        };

        // --- evidence log ---------------------------------------------------
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log_path = base_dir.join(format!("logs/spike/f2-{epoch}.jsonl"));
        let gamepads = flint_input_capture::connected_gamepads().unwrap_or_default();
        let header = serde_json::json!({
            "t": "header", "spike": "f2-player-integration", "epoch_s": epoch,
            "manifest": manifest_path.display().to_string(),
            "chart": chart_path.display().to_string(),
            "latency_ms": latency_ms.unwrap_or(0.0),
            "calibration_ms": calibration_ms,
            "offset_samples": offset_samples,
            "sample_rate": manifest.sample_rate,
            "gamepads": gamepads,
        });
        let evidence = JsonlWriter::create(&log_path, &header)
            .map_err(|e| anyhow!("{e}"))
            .context("creating spike evidence log")?;
        println!("[music-spike] evidence log: {}", log_path.display());

        let end_sample = session.conductor.sample_at_bar(END_BARS, 0.0);
        let sample_rate = manifest.sample_rate;
        let now = Instant::now();
        Ok(Some(Self {
            session,
            judge,
            records: Vec::new(),
            bridge,
            rx,
            _capture: capture,
            evidence,
            sfx_loaded,
            end_sample,
            sample_rate,
            started: now,
            last_skew_report: now,
            last_sfx: None,
            sfx_fired: 0,
            sfx_failed: 0,
            max_skew_ms: 0.0,
            events_total: 0,
            monotonic_violations: 0,
            last_event_sample: i64::MIN,
            last_lean_sample: None,
            lean_deltas_ms: Vec::new(),
            hits: 0,
            misses: 0,
            spurious: 0,
            abs_err_sum_ms: 0.0,
            pulse_release_pending: false,
            finished: false,
        }))
    }

    /// One player frame. Returns `true` when the run is over.
    pub fn tick(&mut self, input: &mut InputState, audio: &mut AudioEngine) -> bool {
        if self.finished {
            return true;
        }

        // Release last frame's down-sampled pulse press so action edge
        // detection sees a full down/up pair.
        if self.pulse_release_pending {
            input.process_gamepad_button_up(SPIKE_GAMEPAD, "South");
            self.pulse_release_pending = false;
        }

        self.session.pump();
        // Raw clock, never suite time — the bridge fits a line through it.
        self.bridge
            .observe(Instant::now(), self.session.raw_clock_sample());
        let pos = self.session.now();

        // --- drain capture: Judge at full precision + InputState down-sample
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                self.events_total += 1;
                let sample = ev.sample();
                if sample < self.last_event_sample {
                    self.monotonic_violations += 1;
                }
                self.last_event_sample = sample;
                match &ev {
                    InputEvent::Lean(l) => {
                        if let Some(prev) = self.last_lean_sample {
                            self.lean_deltas_ms
                                .push((sample - prev) as f64 / self.sample_rate as f64 * 1000.0);
                        }
                        self.last_lean_sample = Some(sample);
                        input.process_gamepad_axis(SPIKE_GAMEPAD, "LeftStickX", l.x as f32);
                        input.process_gamepad_axis(SPIKE_GAMEPAD, "LeftStickY", l.y as f32);
                    }
                    InputEvent::Pulse(_) => {
                        input.process_gamepad_button_down(SPIKE_GAMEPAD, "South");
                        self.pulse_release_pending = true;
                    }
                }
                // No seams in the spike: timeline_offset stays 0, so raw
                // stamps are already suite samples.
                if sample >= 0 {
                    self.judge.ingest(&ev, &mut self.records);
                }
            }
        }
        if pos.sample >= 0 {
            self.judge.advance_to(pos.sample, &mut self.records);
        }
        for rec in self.records.drain(..) {
            match &rec {
                JudgmentRecord::Pulse { err_ms, sample, .. } => {
                    self.hits += 1;
                    self.abs_err_sum_ms += err_ms.abs();
                    println!("[music-spike]   pulse {err_ms:+.1} ms");
                    let _ = self.evidence.write_value(&serde_json::json!({
                        "t": "pulse", "suite_sample": sample, "err_ms": err_ms,
                    }));
                }
                JudgmentRecord::Miss { .. } => self.misses += 1,
                JudgmentRecord::Spurious { .. } => self.spurious += 1,
                JudgmentRecord::Track { .. } => {}
            }
        }

        // --- skew evidence (once per second) --------------------------------
        let mut skew_ms = self.session.mixer.max_stem_skew() * 1000.0;
        let during_sfx = self
            .last_sfx
            .is_some_and(|at| at.elapsed().as_secs_f64() < 1.0);
        // Any nonzero instantaneous reading is investigated the moment it
        // happens: an immediate re-read separates real de-sync (persists)
        // from a handle-position read race (one stem's reported position
        // updating a moment later than another's — vanishes). Only the
        // confirmed value feeds the headline max.
        if skew_ms > 0.5 {
            let reread_ms = self.session.mixer.max_stem_skew() * 1000.0;
            println!(
                "[music-spike]   skew spike {skew_ms:.3} ms (re-read {reread_ms:.3} ms) at raw sample {}{}",
                self.session.raw_clock_sample(),
                if during_sfx { " (during sfx)" } else { "" }
            );
            let _ = self.evidence.write_value(&serde_json::json!({
                "t": "skew_spike", "raw_sample": self.session.raw_clock_sample(),
                "skew_ms": skew_ms, "reread_ms": reread_ms, "during_sfx": during_sfx,
            }));
            skew_ms = skew_ms.min(reread_ms);
        }
        self.max_skew_ms = self.max_skew_ms.max(skew_ms);
        if self.last_skew_report.elapsed().as_secs_f64() >= 1.0 {
            self.last_skew_report = Instant::now();
            println!(
                "[music-spike] bar {} beat {:.0} | skew {skew_ms:.3} ms (max {:.3}) | events {}{}",
                pos.bar,
                pos.beat_in_bar + 1.0,
                self.max_skew_ms,
                self.events_total,
                if during_sfx { " | during sfx" } else { "" }
            );
            let _ = self.evidence.write_value(&serde_json::json!({
                "t": "skew", "raw_sample": self.session.raw_clock_sample(),
                "skew_ms": skew_ms, "max_skew_ms": self.max_skew_ms,
                "during_sfx": during_sfx,
            }));
        }

        // --- contention one-shot (every 2 s, off to the listener's side) ---
        if self.sfx_loaded
            && pos.sample >= 0
            && self
                .last_sfx
                .is_none_or(|at| at.elapsed().as_secs_f64() >= SFX_PERIOD_S)
        {
            self.last_sfx = Some(Instant::now());
            let ok = match audio.play_at_position("spike_sfx", Vec3::new(2.0, 1.0, 0.0), 1.0) {
                Ok(()) => {
                    self.sfx_fired += 1;
                    true
                }
                Err(e) => {
                    self.sfx_failed += 1;
                    eprintln!("[music-spike] *** spatial one-shot FAILED: {e}");
                    false
                }
            };
            let _ = self.evidence.write_value(&serde_json::json!({
                "t": "sfx", "raw_sample": self.session.raw_clock_sample(), "ok": ok,
            }));
        }

        // --- end of run ------------------------------------------------------
        if pos.sample >= self.end_sample || self.session.mixer.all_stopped() {
            self.write_summary();
            self.finished = true;
            return true;
        }
        false
    }

    fn write_summary(&mut self) {
        let dist = summarize(&mut self.lean_deltas_ms);
        let mean_abs_err_ms = if self.hits > 0 {
            self.abs_err_sum_ms / self.hits as f64
        } else {
            0.0
        };
        let summary = serde_json::json!({
            "t": "summary",
            "run_secs": self.started.elapsed().as_secs_f64(),
            "max_skew_ms": self.max_skew_ms,
            "hits": self.hits, "misses": self.misses, "spurious": self.spurious,
            "mean_abs_err_ms": mean_abs_err_ms,
            "lean_deltas_ms": dist,
            "events_total": self.events_total,
            "monotonic_violations": self.monotonic_violations,
            "sfx_fired": self.sfx_fired, "sfx_failed": self.sfx_failed,
        });
        let _ = self.evidence.write_value(&summary);
        let _ = self.evidence.flush();
        println!("[music-spike] SUMMARY {summary}");
        println!(
            "[music-spike] pass checks: max skew {:.3} ms | events {} | monotonic violations {} | sfx {}/{} ok",
            self.max_skew_ms,
            self.events_total,
            self.monotonic_violations,
            self.sfx_fired,
            self.sfx_fired + self.sfx_failed,
        );
    }
}

/// Min/median/p99/max/distinct over the inter-lean-event deltas — the
/// granularity evidence (same bar as `play-chart --spike-input-secs`).
fn summarize(deltas: &mut [f64]) -> serde_json::Value {
    if deltas.is_empty() {
        return serde_json::json!({ "count": 0 });
    }
    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = deltas.len();
    let distinct = {
        let mut d = deltas.to_vec();
        d.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        d.len()
    };
    serde_json::json!({
        "count": n,
        "min": deltas[0],
        "median": deltas[n / 2],
        "p99": deltas[(n as f64 * 0.99) as usize % n],
        "max": deltas[n - 1],
        "distinct": distinct,
    })
}
