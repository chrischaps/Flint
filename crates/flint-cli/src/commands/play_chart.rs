//! Play a suite against its beatmap chart with live input capture: the
//! Phase 2 dev harness. Validates manifest and chart, starts every bus
//! sample-locked, spawns the 1 kHz capture thread, and shows the shared
//! status readout. Judgment, coherence, and session recording hang off this
//! loop as they land. Dev-only surface — never part of a player-facing build.

use anyhow::{bail, Context, Result};
use flint_music::chart_eval::ChartEval;
use flint_music::clock_bridge::ClockBridge;
use flint_music::conductor::Conductor;
use flint_music::input_stream::InputEvent;
use flint_music::judgment::{Judge, JudgmentConfig, JudgmentRecord, JsonlWriter};
use flint_music::session::RealtimePlayer;
use flint_music::status::status_line;
use flint_music::{validate_chart, validate_manifest, validate_manifest_assets, Chart, SuiteManifest};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::latency_files::{latest_calibration_ms, latest_latency_ms};

pub struct PlayChartArgs {
    pub manifest: String,
    pub chart: String,
    pub base_dir: Option<String>,
    pub bars: Option<u64>,
    /// Run the input-granularity spike for this many seconds and exit
    /// (wiggle the stick; no audio involved).
    pub spike_input_secs: Option<u64>,
}

pub fn run(args: PlayChartArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    if let Some(secs) = args.spike_input_secs {
        return run_spike(&base_dir, secs);
    }

    // --- validate both files first ------------------------------------------
    let manifest = SuiteManifest::load(Path::new(&args.manifest))
        .with_context(|| format!("loading {}", args.manifest))?;
    let chart = Chart::load(Path::new(&args.chart))
        .with_context(|| format!("loading {}", args.chart))?;
    let mut issues = validate_manifest(&manifest);
    issues.extend(validate_manifest_assets(&manifest, &base_dir));
    issues.extend(validate_chart(&chart, &manifest));
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("{i}");
        }
        bail!("validation failed ({} issue(s)); not playing", issues.len());
    }

    // --- offsets -------------------------------------------------------------
    let latency_ms = match latest_latency_ms(&base_dir) {
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
    let calibration_ms = match latest_calibration_ms(&base_dir) {
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

    // --- session + judgment --------------------------------------------------
    let mut player = RealtimePlayer::start(&manifest, &base_dir, latency_ms)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    // Judgment arithmetic is pure: event samples arrive already compensated
    // (capture subtracts the offset), so its conductor carries none.
    let judge_conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &judge_conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
    let judgment_cfg = JudgmentConfig::default();
    let mut judge = Judge::new(eval, judge_conductor, judgment_cfg);

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
        "epoch_s": epoch,
    });
    let mut log = JsonlWriter::create(&log_path, &header).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("judgment log: {}", log_path.display());

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

    // --- main loop -----------------------------------------------------------
    let mut last_bar = i64::MIN;
    let mut lean = [0.0f64; 2];
    let mut records = Vec::new();
    let (mut hits, mut misses, mut spurious) = (0u64, 0u64, 0u64);
    let mut abs_err_sum_ms = 0.0f64;
    loop {
        std::thread::sleep(Duration::from_millis(2));
        player.session.pump();
        bridge.observe(Instant::now(), player.session.clock_sample());

        let pos = player.session.now();

        if let Some(rx) = &input_rx {
            while let Ok(ev) = rx.try_recv() {
                if let InputEvent::Lean(l) = &ev {
                    lean = [l.x, l.y];
                }
                if ev.sample() >= 0 {
                    judge.ingest(&ev, &mut records);
                }
            }
        }
        if pos.sample >= 0 {
            judge.advance_to(pos.sample, &mut records);
        }
        for rec in records.drain(..) {
            match &rec {
                JudgmentRecord::Pulse { err_ms, .. } => {
                    hits += 1;
                    abs_err_sum_ms += err_ms.abs();
                    println!("  pulse {err_ms:+.1} ms");
                }
                JudgmentRecord::Miss { .. } => misses += 1,
                JudgmentRecord::Spurious { .. } => spurious += 1,
                JudgmentRecord::Track { .. } => {}
            }
            log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        if pos.sample < 0 {
            continue; // pre-roll
        }
        if pos.sample >= end_sample || player.session.mixer.all_stopped() {
            break;
        }

        if pos.bar != last_bar {
            last_bar = pos.bar;
            let section = player.session.conductor.section_at_sample(pos.sample);
            println!(
                "{} | lean ({:+.2},{:+.2})",
                status_line(&pos, section, &player.session.mixer),
                lean[0],
                lean[1]
            );
            log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    // --- wrap up -------------------------------------------------------------
    drop(capture_handle); // stop the capture thread before the audio manager
    judge.finish(&mut records);
    for rec in records.drain(..) {
        match &rec {
            JudgmentRecord::Miss { .. } => misses += 1,
            JudgmentRecord::Spurious { .. } => spurious += 1,
            _ => {}
        }
        log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mean_ms = if hits > 0 {
        abs_err_sum_ms / hits as f64
    } else {
        0.0
    };
    println!(
        "done. pulses hit {hits} (mean |err| {mean_ms:.1} ms), missed {misses}, spurious {spurious}"
    );
    println!("judgment log: {}", log_path.display());
    Ok(())
}

fn run_spike(base_dir: &Path, secs: u64) -> Result<()> {
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
